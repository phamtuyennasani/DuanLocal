#!/bin/sh
# devstack spike — verify the brew + fpm + dnsmasq pipeline on macOS.
# Run with sudo for the resolver part, but we split it:
#   ./scripts/spike.sh        — installs, writes configs, starts services (non-root)
#   sudo ./scripts/spike.sh   — just the DNS resolver bit
#
# If run as root we only do the resolver and exit.

set -e

TLD="test"
DNS_PORT="5353"
DEVSTACK_DIR="$HOME/.devstack"

if [ "$(id -u)" = "0" ]; then
    echo "==> sudo mode: writing /etc/resolver/$TLD"
    mkdir -p /etc/resolver
    cat > "/etc/resolver/$TLD" <<EOF
nameserver 127.0.0.1
port $DNS_PORT
EOF
    echo "    done. Flush DNS: dscacheutil -flushcache && killall -HUP mDNSResponder"
    exit 0
fi

echo "==> devstack spike (non-root)"
echo "    DEVSTACK_DIR=$DEVSTACK_DIR"

# 1. brew deps
for f in httpd mariadb dnsmasq; do
    brew list "$f" >/dev/null 2>&1 || brew install "$f"
done
brew tap shivammathur/php >/dev/null 2>&1 || true
brew list shivammathur/php/php@8.3 >/dev/null 2>&1 || brew install shivammathur/php/php@8.3

PREFIX=$(brew --prefix)
echo "    brew prefix: $PREFIX"

# 2. dirs
mkdir -p "$DEVSTACK_DIR"/{etc,run,logs,data/mysql,certs}
mkdir -p "$DEVSTACK_DIR/etc/vhosts.d"
mkdir -p "$HOME/Sites/demo"

echo "<?php phpinfo();" > "$HOME/Sites/demo/index.php"

# 3. dnsmasq conf (non-root port 5353)
cat > "$DEVSTACK_DIR/etc/dnsmasq.conf" <<EOF
address=/.$TLD/127.0.0.1
port=$DNS_PORT
listen-address=127.0.0.1
bind-interfaces
EOF

# 4. php-fpm pool
PHP_VER="8.3"
FPM_BIN="$PREFIX/opt/php@$PHP_VER/sbin/php-fpm"
cat > "$DEVSTACK_DIR/etc/php-fpm-$PHP_VER.conf" <<EOF
[global]
pid = $DEVSTACK_DIR/run/php-fpm-$PHP_VER.pid
error_log = $DEVSTACK_DIR/logs/php-fpm-$PHP_VER.log
daemonize = no

[www]
listen = $DEVSTACK_DIR/run/php-fpm-$PHP_VER.sock
listen.owner = $USER
listen.mode = 0666
user = $USER
group = staff
pm = ondemand
pm.max_children = 5
pm.process_idle_timeout = 10s
EOF

# 5. vhost
cat > "$DEVSTACK_DIR/etc/vhosts.d/demo.conf" <<EOF
<VirtualHost *:8080>
    ServerName demo.$TLD
    DocumentRoot "$HOME/Sites/demo"
    <Directory "$HOME/Sites/demo">
        Options Indexes FollowSymLinks
        AllowOverride All
        Require all granted
    </Directory>
    <FilesMatch \.php$>
        SetHandler "proxy:unix:$DEVSTACK_DIR/run/php-fpm-$PHP_VER.sock|fcgi://localhost"
    </FilesMatch>
    ErrorLog "$DEVSTACK_DIR/logs/demo-error.log"
</VirtualHost>
EOF

# 6. minimal httpd.conf
HTTPD_ROOT="$PREFIX/opt/httpd"
MOD="$HTTPD_ROOT/lib/httpd/modules"
{
    echo "ServerRoot \"$HTTPD_ROOT\""
    echo "Listen 8080"
    for m in mpm_event dir mime log_config authz_core authz_host unixd alias rewrite proxy proxy_fcgi socache_shmcb setenvif headers negotiation autoindex reqtimeout env; do
        so="$MOD/mod_$m.so"
        [ -f "$so" ] && echo "LoadModule ${m}_module $so"
    done
    echo "ServerName localhost"
    echo "PidFile \"$DEVSTACK_DIR/run/httpd.pid\""
    echo "ErrorLog \"$DEVSTACK_DIR/logs/httpd-error.log\""
    echo "LogLevel warn"
    echo "User $USER"
    echo "Group staff"
    echo "<Directory />"
    echo "    AllowOverride none"
    echo "    Require all denied"
    echo "</Directory>"
    echo "DirectoryIndex index.php index.html"
    echo "Include \"$DEVSTACK_DIR/etc/vhosts.d/*.conf\""
} > "$DEVSTACK_DIR/etc/httpd.conf"

# 7. start everything
echo "==> starting dnsmasq (needs sudo for port <1024? no — 5353 is fine)"
dnsmasq --conf-file="$DEVSTACK_DIR/etc/dnsmasq.conf" \
    --keep-in-foreground \
    --pid-file="$DEVSTACK_DIR/run/dnsmasq.pid" &
sleep 1

echo "==> starting php-fpm $PHP_VER"
"$FPM_BIN" --fpm-config "$DEVSTACK_DIR/etc/php-fpm-$PHP_VER.conf" &
sleep 1

echo "==> starting httpd"
"$HTTPD_ROOT/bin/httpd" -f "$DEVSTACK_DIR/etc/httpd.conf" -k start

echo "==> done."
echo ""
echo "If this is your first run, also run:  sudo $0"
echo "Then test:  curl -s http://demo.$TLD:8080 | head"
echo "Stop all:   $0 --stop"
