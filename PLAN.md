# DevStack — Kế hoạch xây dựng local dev environment manager

> Mục tiêu: ứng dụng kiểu Laragon/MAMP — quản lý Apache, MariaDB/MySQL, multi-PHP, tên miền ảo — chạy trên macOS và Windows, nhẹ, ít tốn RAM.

## Quyết định nền tảng

| Hạng mục | Quyết định | Lý do |
|---|---|---|
| Shell/UI | **Tauri + Rust** | App ~10–20MB, RAM 30–60MB — gần độ nhẹ native của Laragon |
| Nền tảng trước | **macOS trước**, Windows sau (Phase 4) | User dev chủ yếu trên Mac |
| Phạm vi | **Tool cá nhân** | Không cần auto-update, signing, licensing — giữ scope gọn |
| Runtime | **Native binaries, không Docker** | Docker-based stack tốn 1–2GB RAM; target là ~200–400MB |

## 1. Kiến trúc tổng thể

```
┌─────────────────────────────────────────────┐
│  Tauri App (tray icon + dashboard window)   │
│  Frontend: Svelte — webview hệ thống        │
├─────────────────────────────────────────────┤
│  devctl CLI (binary Rust độc lập)           │
├─────────────────────────────────────────────┤
│  core — Rust library (toàn bộ logic)        │
│  ├── services: spawn/monitor/restart        │
│  ├── sites: site CRUD + docroot             │
│  ├── vhost: render Apache conf templates    │
│  ├── versions: PHP version registry         │
│  ├── dns: wildcard *.test resolver          │
│  ├── certs: mkcert HTTPS                    │
│  └── provider: trait OS-abstract            │
│      (MacProvider / WinProvider sau này)    │
├─────────────────────────────────────────────┤
│  Native binaries:                           │
│  httpd · php-fpm ×N · mariadbd · dnsmasq    │
└─────────────────────────────────────────────┘
```

**Quyết định quan trọng nhất:** tách toàn bộ logic vào `core` lib + CLI `devctl`. UI Tauri chỉ là shell gọi core.

- MVP dùng được ngay bằng CLI, không cần chờ UI
- Bug UI không làm chết services
- Dễ test, dễ port Windows sau này

## 2. Quyết định kỹ thuật trên macOS

| Vấn đề | Giải pháp | Ghi chú |
|---|---|---|
| Nguồn binary | **Homebrew làm provider**: `httpd`, `mariadb`, `dnsmasq`, `shivammathur/php/php@8.x` — chạy với config/data dir riêng 100%, không dùng `brew services` | Brew chỉ là nơi lấy binary (cách Laravel Valet). Long-term: cân nhắc static-php-cli (cách Herd) để portable hoàn toàn |
| PHP multi-version | **php-fpm mỗi version**, listen unix socket `run/php-fpm-8.3.sock`, `pm=ondemand` | Ondemand = gần 0 RAM khi idle |
| Gán PHP per-site | Apache `mod_proxy_fcgi` + `FilesMatch` trỏ socket của version tương ứng | Mỗi site chọn PHP riêng — tốt hơn Laragon (switch global). Điểm bán hàng |
| Tên miền ảo | **dnsmasq wildcard `*.test` → 127.0.0.1** + `/etc/resolver/test` | Thêm site = chỉ tạo vhost, không đụng /etc/hosts nữa. Sudo đúng 1 lần lúc setup |
| Port 80 | Mặc định Apache **8080** (`site.test:8080`). Optional: pfctl redirect 80→8080 | Tránh chạy app bằng root |
| Quyền root | Gom hết vào **`devctl setup` chạy sudo 1 lần** (resolver, dnsmasq daemon, optional pf) | Sau setup app chạy hoàn toàn non-root |
| MariaDB | `mariadbd` datadir riêng trong app dir, `innodb_buffer_pool_size=128M` | Nhẹ hơn MySQL; pool nhỏ đủ cho local dev |

## 3. Cấu trúc thư mục (macOS)

```
~/.devstack/                  # mọi state — xóa thư mục này = uninstall sạch
├── devstack.toml             # config tổng
├── etc/
│   ├── httpd.conf            # Include vhosts.d/*.conf
│   ├── php/8.3/php.ini       # php.ini per version
│   ├── php-fpm/8.3.conf
│   └── my.cnf
├── vhosts.d/                 # auto-generated, 1 file/site
├── run/                      # pid, sockets
├── logs/                     # httpd, php-fpm, mariadb, app
├── data/mysql/               # datadir
├── certs/                    # mkcert CA + per-site certs
└── sites.json                # registry: site → docroot, php version
```

Docroot mặc định `~/Sites/` nhưng user trỏ đâu cũng được.

## 4. Roadmap

### Phase 0 — Spike thủ công (2–3 ngày)
Chứng minh pipeline chạy được trước khi viết code.

- [ ] `brew install httpd mariadb dnsmasq shivammathur/php/php@8.3`
- [ ] Tự tay viết httpd.conf tối thiểu + php-fpm conf + 1 vhost trỏ socket
- [ ] Setup `/etc/resolver/test` + dnsmasq wildcard `*.test`
- [ ] Verify: `curl http://demo.test:8080` render phpinfo đúng version
- [ ] Ghi chú các bẫy: Apple Silicon `/opt/homebrew` vs Intel `/usr/local`, module paths, quyền socket dir
- Deliverable: shell script spike + file notes

### Phase 1 — core lib + `devctl` CLI (1–2 tuần)
MVP thực sự.

```bash
devctl setup                 # sudo 1 lần: resolver, dnsmasq, init datadir
devctl up / down / status    # start/stop services
devctl site add demo --php 8.3 --root ~/code/demo/public
devctl site list / remove
devctl php list              # detect các php@* đã cài
devctl logs apache -f
```

Rust workspace:
```
devstack/
├── Cargo.toml          # workspace
├── crates/
│   ├── core/           # toàn bộ logic
│   ├── cli/            # devctl binary
│   └── app/            # Tauri (Phase 2)
└── scripts/            # spike scripts
```

### Phase 2 — Tauri UI (1–2 tuần)
- Tray icon: quick start/stop, open site
- Dashboard: service status + toggle, bảng sites (domain, docroot, dropdown PHP version, open/logs), log viewer streaming, settings (port, autostart, RAM profile)

### Phase 3 — Polish
- mkcert HTTPS per site (1 checkbox)
- Mailpit (mail catcher)
- Adminer one-file bundle (nhẹ hơn phpMyAdmin)
- RAM profiles (`ondemand`/`static` presets)
- Import folder `www` sẵn có
- Auto-start on login

### Phase 4 — Windows port
Core đã abstract qua trait `PlatformProvider`:
- WinProvider: zip binaries (ApacheLounge httpd, windows.php.net PHP NTS, MariaDB zip)
- Sửa `hosts` file thay dnsmasq (Laragon-style — elevation chỉ khi ghi)
- Spawn process qua `std::process` giữ nguyên

## 5. RAM budget (ước tính, macOS)

| Component | Idle | Active |
|---|---|---|
| Tauri app (webview) | ~50MB | ~80MB |
| Apache `mpm_event` | ~15MB | ~40MB |
| php-fpm mỗi version (`ondemand`) | ~10MB | ~50–80MB |
| MariaDB (pool 128M) | ~120MB | ~150MB |
| dnsmasq | ~3MB | ~3MB |
| **Tổng (2 PHP versions)** | **~210MB** | **~400MB** |

So sánh: Docker stack thường 1–2GB; riêng Electron shell đã 150–300MB.

## 6. Rủi ro chính

- **Brew update phá paths** → detect động qua `brew --prefix`, không hardcode; `devctl php list` rescan mỗi lần
- **PHP versions cũ bị brew drop** → binary cũ vẫn chạy được vì config/data của mình; long-term cân nhắc static-php-cli
- **macOS SIP/permissions** → gom sudo vào 1 chỗ, test trên máy sạch
- **Tham khảo:** Laravel Valet (brew + dnsmasq trên Mac), Laravel Herd (static PHP + UX), Laragon (feature checklist, Windows later)

## Bước tiếp theo

Bắt đầu **Phase 0**: viết shell script spike cho macOS (cài brew formulas, generate config tối thiểu, tạo 1 site demo). Khi chạy OK trên máy Mac → scaffold repo Rust workspace (Phase 1).
