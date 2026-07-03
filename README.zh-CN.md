# photondns

[English](README.md) | **简体中文**

OpenWrt 高性能 DNS 转发器，使用 Rust 编写。以 [mosdns](https://github.com/sbwml/luci-app-mosdns)
的功能为蓝本从零重写，专注于极致速度与**永不让客户端等待的故障转移**，并配有完整的
LuCI 管理界面（`luci-app-photondns`，中英双语）。

单个静态二进制，**2.3 MB**，运行内存 **约 5 MB**。

## 实测性能（Ariaboard photonicat2，rockchip aarch64）

| 指标 | 结果 |
|---|---|
| 设备本机缓存查询 | **约 90,000 qps**，平均 0.35 ms |
| 缓存命中延迟 | 0 ms（dig），TTL 正确递减 |
| 所有主上游宕机时的冷启动查询 | **285 ms** 内通过对冲备用上游应答 |
| 主上游宕机稳态（熔断器打开后） | **12 ms** |
| 内存 | 5.2 MB RSS |

## 为什么快

- **零重编码转发**：查询以原始报文字节转发（仅改写 2 字节 ID）；缓存应答直接字节复制，
  就地修补 ID/TTL/问题区。全程没有 DNS 报文重新序列化。
- **分片 LRU 缓存**（16 分片，`parking_lot` 锁），大小可配置；高并发下微秒级查找。
- **上游连接复用**：TCP 与 DoT 使用 RFC 7766 查询流水线复用长连接；DoH 使用
  HTTP/1.1 keep-alive 连接池；UDP 共享套接字按 ID 解复用。
- Linux 下多套接字 UDP 监听（`SO_REUSEPORT`）。

## 故障转移（生而"极速"）

每个查询都经过*对冲执行引擎*：

1. 上游按健康状态 + EWMA 延迟排序（每上游独立、无锁统计）。
2. 先询问最优上游。若在**自适应对冲延迟**（约为最优上游 EWMA 的 2 倍，上限
   `hedge_delay_ms`）内没有应答，则*并行*竞速次优上游，最先返回的有效应答获胜。
3. 任何硬失败立即触发下一个候选。
4. **备用上游挂在候选队尾**：即使所有主上游宕机的冷启动查询也能在一个对冲间隔内
   得到应答——没有 SERVFAIL，没有超时等待。
5. **熔断器**（连续 N 次失败 → 下线，冷却 → 半开 → M 次成功 → 恢复）把死上游移出
   轮换；**主动健康探测**保持延迟统计新鲜、空闲时也能发现故障、并为 TLS 连接保温。
6. 上游 UDP 应答被截断时自动改用 TCP 重试（查询方式回退）。
7. 若*全部*失败，则以过期缓存条目兜底应答。

策略：`race`（默认）、`fastest`、`parallel`、`sequential`、`random`。

## 对齐 luci-app-mosdns 的功能，并有所超越

- UDP + TCP 监听，地址/端口可配置
- 上游：`udp://`、`tcp://`、`tls://`（DoT）、`https://`（DoH），支持 bootstrap
  解析 DoT/DoH 域名及 `insecure_skip_verify`
- **可配置缓存大小**，最小/最大 TTL 钳制，负应答 TTL
- **过期应答**（惰性缓存）+ 热门条目到期前**预取**
- **缓存持久化**（定期 + 关机时保存，启动时恢复）
- 规则文件：hosts、拦截列表（NXDOMAIN）、重定向、本地域名分流到独立 "local"
  上游分组
- **国内外分流解析**：一键下载 dnsmasq-china-list（约 11 万域名，优先国内镜像）；
  中国大陆域名走本地域名 DNS 分组，其余走主分组——`/stats` 中的分组查询计数
  实时展示分流效果
- 可选拦截 HTTPS/SVCB type-65 查询
- 内置防护：`.local`/`.lan` 等 RFC-6761 特殊域名与私网 PTR 反查在本地直接返回
  NXDOMAIN，不再泄漏到上游
- **广告拦截**：自动下载列表（anti-AD、Cats-Team AdRules、hosts 格式等）并
  返回 NXDOMAIN，附 LuCI 更新页面与状态显示
- **实时查询日志**（LuCI）：内存中保留最近 N 条查询（默认 5000），显示客户端、
  域名、应答路径（缓存/过期/hosts/拦截/local/main）、获胜上游与耗时，可过滤、
  自动刷新
- **定时自动更新**（cron）国内域名列表与广告列表
- dnsmasq 接管（`redirect`）与防火墙 DNS 劫持（`dns_hijack`）选项
- HTTP JSON API：`/stats`、`/flush`、`/log`、`/health`、`/version`（仅 127.0.0.1）
- LuCI 界面：实时状态面板（上游健康、EWMA 延迟、对冲次数、缓存命中率）、
  完整设置编辑、规则文件编辑、日志查看，中英双语

## 目录结构

```
src/                    Rust 源码（服务、缓存、上游、故障转移、路由、API）
src/bin/photonbench.rs    轻量 UDP DNS 压测工具
openwrt/photondns/        OpenWrt 软件包 Makefile（SDK 构建）
openwrt/luci-app-photondns/  LuCI 应用：视图、rpcd ucode 后端、ACL、菜单、
                        UCI 配置 + 生成 TOML 的 procd init 脚本、po 翻译
tools/po2lmo.py         po -> lmo 编译器（直接部署用）
deploy.sh               通过 SSH 直接部署到设备
```

## 构建

```sh
cargo test                                              # 单元测试
cargo build --release                                   # 本机构建
cargo zigbuild --release --target aarch64-unknown-linux-musl   # OpenWrt aarch64
```

## 部署到设备

```sh
./deploy.sh root@192.168.1.1
ssh root@192.168.1.1 'uci set photondns.main.enabled=1; uci commit photondns; /etc/init.d/photondns restart'
dig @192.168.1.1 -p 15533 example.com
```

然后打开 LuCI → 服务 → photondns。要将其作为系统解析器，在基本设置中启用
**DNS 转发**（可选再开启 **DNS 重定向**）——dnsmasq 将被重新配置为转发到
photondns（原设置会备份，关闭时自动恢复）。

## 配置

`/etc/config/photondns`（UCI）是配置源，init 脚本自动生成
`/var/etc/photondns.toml`。守护进程也可用手写 TOML 独立运行
（`photondns -c config.toml`，`-t` 校验配置）：

```toml
[server]
listen = ["0.0.0.0:15533"]

[cache]
size = 8192          # 条目数（核心可调项）
serve_stale = true
prefetch = true
dump_file = "/etc/photondns/cache.dump"

[failover]
health_check_interval = 10
fail_threshold = 3
cooldown = 15

[[group]]
name = "main"
strategy = "race"
upstreams = ["udp://223.5.5.5", "udp://119.29.29.29"]
backups = ["tls://8.8.8.8"]
hedge_delay_ms = 250
timeout_ms = 2000
```

## 许可证

GPL-3.0-only。
