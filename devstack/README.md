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

### macOS
The packaged macOS app uses the system Apache binary and downloads MySQL, dnsmasq, mkcert, and requested PHP versions into `~/.devstack/vendor/` as needed. Homebrew is not required for the packaged runtime.

### Windows
Binaries are vendored into `~/.devstack/vendor/` on demand — direct downloads, no manual fetching:
```powershell
devctl install httpd      # ApacheLounge VS18 zip
devctl install mariadb    # archive.mariadb.org zip
devctl install php-8.3    # windows.php.net NTS zip (latest patch, sha256-verified)
devctl install php-8.4    # any 8.x branch upstream offers
```
Or use the **packages** panel in the app — one click per version, progress shown live.

## Build

```sh
cargo build --release
```

**macOS:** `devctl setup` (or open the app and click **Finish setup**) — this shows one native administrator prompt for local DNS and public ports. Do not run it with `sudo`; user-owned state stays under `~/.devstack/`.

## First launch

Install DevStack, open it, then select **Finish setup** in the one-time banner. macOS displays a native administrator prompt that configures local `.test` domains and public port 80. No Terminal command is required. DevStack downloads its user-owned runtimes and configuration into `~/.devstack/` as the signed-in user.

For automation or troubleshooting, `devctl setup` remains available and opens the same native administrator prompt. Do **not** run the full setup with `sudo`, because that can make `~/.devstack/` root-owned.

## Package a macOS application

Build a macOS `.app` and `.dmg` package:

```sh
./scripts/package-macos.sh
```

The script builds `devctl`, includes it in the app bundle, and invokes the Tauri bundler. The default package matches the build machine architecture. Output is written to:

```text
target/<architecture>/release/bundle/macos/DevStack.app
target/<architecture>/release/bundle/dmg/DevStack_0.1.0_<architecture>.dmg
```

On Apple Silicon:

```sh
./scripts/package-macos.sh
```

On an Intel Mac, or when cross-building for Intel:

```sh
rustup target add x86_64-apple-darwin
DEVSTACK_TARGET=x86_64-apple-darwin ./scripts/package-macos.sh
```

The package does not include personal projects or databases. On first launch DevStack stores downloaded runtimes and configuration in `~/.devstack/`. Setup downloads MySQL, dnsmasq, and mkcert. Install PHP separately from the Packages panel. Apache uses the macOS system binary. When DevStack first needs local `.test` domains or public port 80, click **Finish setup** and approve the native administrator prompt; no Terminal command is needed.

Unsigned local builds may trigger macOS Gatekeeper on another Mac. Use **Open** from Finder's context menu, or sign/notarize the app with an Apple Developer identity before distributing it broadly.

## Package a Windows installer

Windows installers are built on a Windows runner because MSVC, the Windows SDK, and the installer toolchain are required. From a Windows checkout:

```powershell
.\scripts\package-windows.ps1
```

The script creates an unsigned NSIS installer under:

```text
target\release\bundle\nsis\
```

The GitHub Actions release workflow builds both the Apple Silicon macOS DMG and the Windows x64 installer, then uploads SHA-256 checksums. Trigger it manually from Actions or push a version tag such as `v0.1.0`.

Windows downloads Apache, MariaDB, and PHP into the per-user `~/.devstack/vendor/` directory on demand. The Windows app uses hosts-file entries instead of macOS wildcard DNS. Creating or removing a site may require running DevStack with administrator privileges. Unsigned installers may trigger SmartScreen; choose **More info → Run anyway** when you trust the build.


```sh
devctl site add demo --php 8.3 --root ~/Sites/demo
devctl up
# open http://demo.test/

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

`~/.devstack/` holds personal DevStack state. Removing it does **not** remove macOS system DNS/firewall integration; open DevStack and disable it through the app before uninstalling, or remove the DevStack resolver, pf anchor, and LaunchDaemon separately.

## Platform differences

| | macOS | Windows |
|---|---|---|
| Binaries | System Apache + vendored MySQL/dnsmasq/PHP | Vendored zips in `~/.devstack/vendor/` (auto-download) |
| PHP handler | php-fpm unix socket | php-cgi via `Action`/`ScriptAlias` (mod_cgi) |
| DNS | dnsmasq wildcard `*.test` + `/etc/resolver` | `hosts` file per-site entries |
| Elevated ops | native administrator prompt once | `devctl site add` writes hosts (admin) |

> **Why CGI on Windows?** `mod_proxy_fcgi` hands php-cgi a `proxy:`-prefixed
> `SCRIPT_FILENAME` that php-cgi (unlike php-fpm) rejects as "No input file
> specified." `Action`/`ScriptAlias` via `mod_actions`+`mod_cgi` supplies the
> real filesystem path — slower per-request than FCGI but bulletproof, and fine
> for a local dev tool.

## Roadmap

- [x] Phase 1: `devctl` CLI, core service management
- [x] Phase 2: Tauri UI (dashboard — services, sites, logs)
- [ ] Phase 3: mkcert HTTPS, Mailpit, Adminer
- [x] Phase 4: Windows port (hosts file + php-cgi + vendored binaries)
