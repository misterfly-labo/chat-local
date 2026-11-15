# Maintainer: Enky
pkgname=chat-local
pkgver=1.0
pkgrel=1
pkgdesc="Interface graphique pour chatter avec les LLM de Lemonade"
arch=('x86_64')
url="https://github.com/enky/chat-local"
license=('MIT')
depends=('openssl' 'fontconfig' 'gcc-libs' 'glibc' 'libx11' 'wayland')
makedepends=('cargo' 'rust')

build() {
    cd "$pkgname"
    cargo build --release
}

package() {
    cd "$pkgname"
    install -Dm755 target/release/chat-local -t "$pkgdir/usr/bin"
    install -Dm644 README.md -t "$pkgdir/usr/share/doc/$pkgname"
    install -Dm644 LICENSE -t "$pkgdir/usr/share/licenses/$pkgname"
}
