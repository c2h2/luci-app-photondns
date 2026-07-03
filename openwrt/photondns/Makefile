include $(TOPDIR)/rules.mk

PKG_NAME:=photondns
PKG_VERSION:=0.1.0
PKG_RELEASE:=1

PKG_LICENSE:=GPL-3.0-only
PKG_MAINTAINER:=c2h2

PKG_BUILD_DEPENDS:=rust/host

include $(INCLUDE_DIR)/package.mk
include $(TOPDIR)/feeds/packages/lang/rust/rust-package.mk

define Package/photondns
  SECTION:=net
  CATEGORY:=Network
  SUBMENU:=IP Addresses and Names
  TITLE:=High-performance DNS forwarder with adaptive failover
  DEPENDS:=$(RUST_ARCH_DEPENDS)
endef

define Package/photondns/description
  DNS forwarder written in Rust: sharded LRU cache with configurable size,
  serve-stale, prefetch, cache persistence, UDP/TCP/DoT/DoH upstreams and
  hedged-racing failover with per-upstream health tracking.
endef

define Package/photondns/install
	$(INSTALL_DIR) $(1)/usr/bin
	$(INSTALL_BIN) $(PKG_INSTALL_DIR)/bin/photondns $(1)/usr/bin/
endef

$(eval $(call RustBinPackage,photondns))
$(eval $(call BuildPackage,photondns))
