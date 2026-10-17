#!/usr/bin/env bash
# A throwaway local Dovecot for the live IMAP tests. Runs as you (no root, no
# system service), listens on 127.0.0.1 only, keeps everything in target/dovecot.
#
#   dev/test-imap-server.sh start   # create (first time) and start
#   dev/test-imap-server.sh stop
#   dev/test-imap-server.sh reset   # stop and delete all its mail and config
#   eval "$(dev/test-imap-server.sh env)"   # TERN_IMAP_* and SSL_CERT_FILE for the tests
#
# Then:  cargo test -p tern-imap --test live -- --ignored --nocapture
# Needs: dovecot 2.4, openssl.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIR="$ROOT/target/dovecot"
CONF="$DIR/dovecot.conf"
USER_NAME="tom"
PASSWORD="tern-test-password"
IMAP_PORT=10143
IMAPS_PORT=10993

setup() {
    mkdir -p "$DIR"/{run,state,home}
    if [[ ! -f "$DIR/server.crt" ]]; then
        # A test CA plus a localhost certificate it signs (rustls won't accept a
        # self-signed leaf). The tests trust only this CA via SSL_CERT_FILE.
        openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -subj "/CN=Tern test CA" \
            -keyout "$DIR/ca.key" -out "$DIR/ca.crt" \
            -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign" 2>/dev/null
        openssl req -newkey rsa:2048 -nodes -subj "/CN=localhost" \
            -keyout "$DIR/server.key" -out "$DIR/server.csr" 2>/dev/null
        printf 'subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n' \
            > "$DIR/server.ext"
        openssl x509 -req -in "$DIR/server.csr" -CA "$DIR/ca.crt" -CAkey "$DIR/ca.key" -CAcreateserial \
            -days 3650 -extfile "$DIR/server.ext" -out "$DIR/server.crt" 2>/dev/null
    fi
    printf '%s:{PLAIN}%s:%s:%s::%s\n' "$USER_NAME" "$PASSWORD" "$(id -u)" "$(id -g)" "$DIR/home/$USER_NAME" \
        > "$DIR/users"
    cat > "$CONF" <<EOF
dovecot_config_version = 2.4.5
dovecot_storage_version = 2.4.5

base_dir = $DIR/run
state_dir = $DIR/state
log_path = $DIR/dovecot.log
listen = 127.0.0.1
protocols {
  imap = yes
}

# Run every process as the invoking user; no chroot (needs root).
default_internal_user = $(id -un)
default_internal_group = $(id -gn)
default_login_user = $(id -un)
service imap-login {
  chroot =
  inet_listener imap {
    port = $IMAP_PORT
  }
  inet_listener imaps {
    port = $IMAPS_PORT
    ssl = yes
  }
}
service anvil {
  chroot =
}
service stats {
  chroot =
}

ssl = yes
ssl_server {
  cert_file = $DIR/server.crt
  key_file = $DIR/server.key
}

auth_mechanisms = plain login
passdb passwd-file {
  passwd_file_path = $DIR/users
}
userdb passwd-file {
  passwd_file_path = $DIR/users
}

mail_driver = maildir
mail_home = $DIR/home/%{user}
mail_path = ~/Maildir

namespace inbox {
  inbox = yes
  separator = /
  mailbox Drafts {
    special_use = \\Drafts
    auto = subscribe
  }
  mailbox Sent {
    special_use = \\Sent
    auto = subscribe
  }
  mailbox Trash {
    special_use = \\Trash
    auto = subscribe
  }
  mailbox Junk {
    special_use = \\Junk
    auto = subscribe
  }
  mailbox Archive {
    special_use = \\Archive
    auto = subscribe
  }
}
EOF
}

running() {
    [[ -f "$DIR/run/master.pid" ]] && kill -0 "$(cat "$DIR/run/master.pid")" 2>/dev/null
}

case "${1:-}" in
    start)
        setup
        if running; then
            echo "already running"
        else
            # Detach from our stdout/stderr (the master keeps them open); errors go to the log.
            dovecot -c "$CONF" </dev/null >>"$DIR/dovecot.log" 2>&1
            echo "Dovecot running on 127.0.0.1:$IMAPS_PORT (TLS) and :$IMAP_PORT (STARTTLS); log: $DIR/dovecot.log"
        fi
        ;;
    stop)
        if running; then
            doveadm -c "$CONF" stop
        else
            echo "not running"
        fi
        ;;
    reset)
        if running; then doveadm -c "$CONF" stop; fi
        rm -rf "$DIR"
        ;;
    env)
        echo "export TERN_IMAP_HOST=localhost TERN_IMAP_USER=$USER_NAME TERN_IMAP_PASS=$PASSWORD" \
             "TERN_IMAP_PORT=$IMAPS_PORT TERN_IMAP_STARTTLS_PORT=$IMAP_PORT SSL_CERT_FILE=$DIR/ca.crt"
        ;;
    *)
        sed -n '2,10p' "$0"
        exit 1
        ;;
esac
