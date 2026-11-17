# Maintainer: Tom
pkgname=tern-git
pkgver=0.1.0
pkgrel=1
pkgdesc="Apple Mail-style email client for Linux (GTK4 + libadwaita)"
arch=('x86_64' 'aarch64')
url="https://github.com/usiel667/tern"
license=('GPL-3.0-or-later')
depends=('gtk4' 'libadwaita' 'webkitgtk-6.0' 'hicolor-icon-theme'
         'glib2' 'gdk-pixbuf2' 'pango' 'cairo' 'graphene' 'glibc' 'libgcc')
optdepends=('gnome-keyring: store account passwords'
            'keepassxc: alternative Secret Service provider')
makedepends=('cargo' 'git')
# makepkg's LTO builds the C in aws-lc-rs (via reqwest's rustls) as LTO objects that
# Rust's linker can't use: undefined aws_lc_* symbols in a clean chroot (2026-11-17).
options=('!lto')
# Not provides=('tern'): the AUR's "tern" is an unrelated PostgreSQL migration tool. Both
# install /usr/bin/tern, hence the conflict.
conflicts=('tern')
source=("$pkgname::git+$url.git")
sha256sums=('SKIP')

pkgver() {
  cd "$pkgname"
  printf "0.1.0.r%s.%s" "$(git rev-list --count HEAD)" "$(git rev-parse --short HEAD)"
}

build() {
  cd "$pkgname"
  export CARGO_TARGET_DIR=target
  cargo build --release --locked
}

check() {
  cd "$pkgname"
  cargo test --release --locked --workspace
}

package() {
  cd "$pkgname"
  install -Dm755 target/release/tern "$pkgdir/usr/bin/tern"
  install -Dm644 crates/tern/data/dev.tern.Tern.desktop "$pkgdir/usr/share/applications/dev.tern.Tern.desktop"
  install -Dm644 crates/tern/data/dev.tern.Tern.svg "$pkgdir/usr/share/icons/hicolor/scalable/apps/dev.tern.Tern.svg"
  install -Dm644 crates/tern/data/dev.tern.Tern.metainfo.xml "$pkgdir/usr/share/metainfo/dev.tern.Tern.metainfo.xml"
}
