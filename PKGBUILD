# Maintainer: Enky
pkgname=chat-local
pkgver=1.0
pkgrel=1
pkgdesc="Interface graphique pour chatter avec les LLM de Lemonade"
arch=('x86_64')
url="https://github.com/misterfly-labo/chat-local"
license=('MIT')
depends=('fontconfig' 'gcc-libs' 'glibc' 'libx11' 'wayland' 'wl-clipboard' 'pipewire')
makedepends=('cargo' 'rust')

build() {
    # PKGBUILD à la racine du dépôt : compiler sur place.
    cd "$startdir"
    export CARGO_TARGET_DIR="$srcdir/target"
    # Les CFLAGS/LDFLAGS de makepkg cassent la compilation des crates C (ring).
    unset CFLAGS CXXFLAGS LDFLAGS CPPFLAGS
    cargo build --release --frozen
}

package() {
    cd "$startdir"
    install -Dm755 "$srcdir/target/release/chat-local" -t "$pkgdir/usr/bin"
    install -Dm644 README.md -t "$pkgdir/usr/share/doc/$pkgname"
    install -Dm644 LICENSE -t "$pkgdir/usr/share/licenses/$pkgname"
    install -Dm644 chat-local.svg "$pkgdir/usr/share/icons/hicolor/scalable/apps/chat-local.svg"
    install -Dm644 chat-local.desktop "$pkgdir/usr/share/applications/chat-local.desktop"
}
