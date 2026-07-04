include $(TOPDIR)/rules.mk

PKG_NAME:=luci-app-photondns-compat
PKG_VERSION:=0.1.0
PKG_RELEASE:=1

PKG_LICENSE:=GPL-3.0-only
PKG_MAINTAINER:=c2h2

LUCI_TITLE:=LuCI support for photondns (legacy Lua/CBI, for old LuCI without ucode/JS client)
LUCI_DEPENDS:=+photondns +curl +luci-compat
LUCI_PKGARCH:=all

include $(TOPDIR)/feeds/luci/luci.mk

# call BuildPackage - OpenWrt buildroot signature
