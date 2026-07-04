# photondns

[English](README.md) | **简体中文**

**Rust** 编写的高性能 DNS 转发器，为 OpenWrt 路由器设计，任何 Linux
环境都能跑得飞快。自带完整的 LuCI Web 界面（`luci-app-photondns`）。

**2.3 MB 静态二进制 · 运行内存约 5 MB · 零重编码转发 · 对冲式故障切换**

## 为什么快

- **零重编码路径** — 查询以原始报文字节转发（仅改写 2 字节 ID）；缓存命中
  直接字节拷贝并原地改写 ID/TTL，全程没有 DNS 报文重新序列化。
- **高并发架构** — tokio 多线程运行时；16 分片 LRU 缓存（`parking_lot`）；
  每上游无锁健康/延迟统计；Linux 下多 socket UDP 监听（`SO_REUSEPORT`）。
- **上游连接复用** — TCP/DoT 走 RFC 7766 持久连接流水线，DoH 走 HTTP
  keep-alive 连接池，UDP 共享 socket 按 ID 解复用。
- **对冲式故障切换** — 按健康度 + EWMA 延迟给上游排序；响应慢就并行竞速
  下一个上游；熔断器 + 主动探测把挂掉的上游踢出轮换。即使所有主上游全挂，
  客户端也不会看到 SERVFAIL。

## 性能实测 — 能到 10 万 QPS 吗？100 万呢？

使用自带的 `photonrbench` 测试（随机域名；冷测 = 全部缓存未命中、走完整
上游路径，热测 = 缓存命中）：

| 平台 | 场景 | 吞吐 | 延迟 |
|---|---|---|---|
| Apple M3 Pro（12 核），回环 | 单 UDP socket，20 万查询 | **约 105,000 qps** | p50 0.6 ms，p99 1.0 ms |
| Apple M3 Pro（12 核），回环 | 8 socket × 8 并行客户端 | 合计 **约 276,000 qps** | p50 1.5 ms |
| Ariaboard photonicat2（4×A55 路由器），本机 | 缓存命中 | **约 90,000 qps** | 平均 0.35 ms |

结论：**10 万+ QPS —— 没问题**，单个 UDP socket 就够，路由器级 ARM 硬件也
接近这个量级。笔记本上实测 **约 27.6 万 QPS**（压测端还和服务端抢同一颗
CPU，纯服务端上限更高）。**100 万 QPS 属于外推，尚未实测**：吞吐随
socket 数线性扩展（`SO_REUSEPORT`），多核服务器 + 独立压测机预计可达。
如果你测到了，欢迎提 issue 附上数据。

复现：

```sh
cargo build --release
./target/release/photondns -c config.toml            # 监听 127.0.0.1:15533
./target/release/photonrbench 127.0.0.1:15533 200000 64
# photonrbench [server:port] [数量] [并发]
# 环境变量：SUFFIX=<真实域名后缀>  WARM=0  SEED=<n>
```

冷测会走真实转发/故障切换路径 —— 除非你想把大量随机域名发给公共 DNS，
否则请把配置指向本地测试上游。

## 功能

- UDP + TCP 监听；上游支持 `udp://`、`tcp://`、`tls://`（DoT）、
  `https://`（DoH），DoT/DoH 域名自动 bootstrap 解析
- 缓存：容量可配、TTL 钳制、**过期兜底（serve-stale）**、**热点预取**、
  重启后持久化
- 故障切换策略：`race`（默认）、`fastest`、`parallel`、`sequential`、
  `random`；自适应对冲延迟、熔断器、主动健康探测
- 规则：hosts、拦截（NXDOMAIN）、重定向、本地域名分流
- **国内 / 国外分流** — 一键下载 dnsmasq-china-list（约 11 万域名），
  大陆域名走本地组，其余走主组
- **广告拦截**，列表自动更新（anti-AD、AdRules、hosts 格式）
- LuCI **实时查询日志**（客户端、域名、路由、上游、延迟）
- 特殊 TLD（`.local`/`.lan`）与内网 PTR 保护，可选拒绝 HTTPS/SVCB
  type-65 查询
- HTTP JSON API：`/stats`、`/flush`、`/log`、`/health`、`/version`
- 双语 LuCI 应用（English / 简体中文）：实时仪表盘、设置、规则编辑器、
  日志查看器；支持 dnsmasq 接管与防火墙 DNS 劫持

## 快速开始

独立运行：

```sh
cargo build --release
./target/release/photondns -c config.toml    # -t 校验配置
```

```toml
[server]
listen = ["0.0.0.0:15533"]

[cache]
size = 8192
serve_stale = true

[[group]]
name = "main"
strategy = "race"
upstreams = ["udp://223.5.5.5", "udp://119.29.29.29"]
backups = ["tls://8.8.8.8"]
```

OpenWrt（通过 SSH 部署二进制 + LuCI 应用）：

```sh
cargo zigbuild --release --target aarch64-unknown-linux-musl
./deploy.sh root@192.168.1.1
ssh root@192.168.1.1 'uci set photondns.main.enabled=1; uci commit photondns; /etc/init.d/photondns restart'
dig @192.168.1.1 -p 15533 example.com
```

然后打开 LuCI → 服务 → photondns。开启 *DNS 转发* 即可接管系统解析
（dnsmasq 原配置会自动备份并在关闭时恢复）。

## 许可证

GPL-3.0-only。
