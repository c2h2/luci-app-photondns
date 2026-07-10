include $(TOPDIR)/rules.mk

PKG_NAME:=luci-app-photondns
PKG_VERSION:=0.2.3
PKG_RELEASE:=1

PKG_LICENSE:=GPL-3.0-only
PKG_MAINTAINER:=c2h2

LUCI_TITLE:=LuCI support for photondns
LUCI_DEPENDS:=+photondns +curl
LUCI_PKGARCH:=all

include $(TOPDIR)/feeds/luci/luci.mk

# call BuildPackage - OpenWrt buildroot signature
