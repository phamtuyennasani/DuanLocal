# devstack

Laragon-style local dev environment manager — macOS first, Windows later. Tauri + Rust, native binaries (no Docker).

## Status

Phase 1 + 2 + 4 scaffold — `devctl` CLI + Tauri UI, macOS + Windows.

## Prereqs

### macOS
```sh
brew install httpd mariadb dnsmasq
brew tap shivammathur/php
brew install shivammathur/php/php@8.3
```

### Windows
Binaries are vendored into `~/.devstack/vendor/` on demand:
```powershell
devctl install httpd      # Apache Lounge zip
devctl install mariadb    # MariaDB zip
devctl install php-8.3    # windows.php.net NTS zip
```

## Build

```sh
cargo build --release
```

**macOS:** `sudo ./target/release/devctl setup` — writes `/etc/resolver/test` + dnsmasq conf
**Windows:** run terminal as Administrator, then `devctl setup` — verifies hosts file is writable; sites write hosts entries per-add

## CLI

```sh
devctl site add demo --php 8.3 --root ~/Sites/demo
devctl up
# open http://demo.test:8080

devctl site list
devctl site set-php demo 8.4
devctl status
devctl logs httpd -f
devctl down
```

## UI

```sh
cargo run --release -p devstack-app
```

## Layout

`~/.devstack/` — delete to uninstall.

## Platform differences

| | macOS | Windows |
|---|---|---|
| Binaries | Homebrew | Vendored zips in `~/.devstack/vendor/` |
| PHP handler | php-fpm unix socket | php-cgi TCP `127.0.0.1:90XY` |
| DNS | dnsmasq wildcard `*.test` + `/etc/resolver` | `hosts` file per-site entries |
| Elevated ops | `sudo devctl setup` once | `devctl site add` writes hosts (admin) |

## Roadmap

- [x] Phase 1: `devctl` CLI, core service management
- [x] Phase 2: Tauri UI (dashboard — services, sites, logs)
- [ ] Phase 3: mkcert HTTPS, Mailpit, Adminer
- [x] Phase 4: Windows port (hosts file + php-cgi + vendored binaries)
