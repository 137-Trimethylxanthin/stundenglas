#!/usr/bin/env bash
# Builds a Debian container on a Proxmox host, puts the stundenglas service in
# it, and leaves it stopped until its configuration is filled in.
#
#   bash -c "$(curl -fsSL https://raw.githubusercontent.com/137-Trimethylxanthin/stundenglas/main/scripts/proxmox-lxc.sh)"
#
# It stands on its own. The community-scripts framework fetches its installer
# from a path hardcoded to that project's own repository, so an application
# living anywhere else cannot borrow it.
set -Eeuo pipefail

REPO="${STUNDENGLAS_REPO:-https://github.com/137-Trimethylxanthin/stundenglas}"
BRANCH="${STUNDENGLAS_BRANCH:-main}"

YW=$'\033[33m' GN=$'\033[1;92m' RD=$'\033[01;31m' BL=$'\033[36m' CL=$'\033[m'
msg()  { echo -e " ${BL}›${CL} $1"; }
ok()   { echo -e " ${GN}✓${CL} $1"; }
warn() { echo -e " ${YW}!${CL} $1"; }
die()  { echo -e " ${RD}✗${CL} $1" >&2; exit 1; }

trap 'die "failed at line $LINENO. The container, if it was made, is left alone for you to look at."' ERR

header() {
    printf '%s\n' "$BL"
    cat <<'ART'
     _                   _                  _
 ___| |_ _   _ _ __   __| | ___ _ __   __ _| | __ _ ___
/ __| __| | | | '_ \ / _` |/ _ \ '_ \ / _` | |/ _` / __|
\__ \ |_| |_| | | | | (_| |  __/ | | | (_| | | (_| \__ \
|___/\__|\__,_|_| |_|\__,_|\___|_| |_|\__, |_|\__,_|___/
                                      |___/   w e b u n t i s
ART
    printf '%s' "$CL"
    echo -e "\n   ${YW}a timetable, as a calendar you can subscribe to${CL}\n"
}

# ── the ground we stand on ───────────────────────────────────────────────────

[[ $EUID -eq 0 ]] || die "run this as root, on the Proxmox host itself"
command -v pct >/dev/null || die "no pct here. This belongs on the Proxmox host, not inside a container"
command -v whiptail >/dev/null || die "whiptail is missing; install libnewt-utils"

PVE_MAJOR=$(pveversion | sed -n 's|^pve-manager/\([0-9]*\).*|\1|p')
[[ ${PVE_MAJOR:-0} -ge 8 ]] || warn "tested on Proxmox 8 and 9; yours reports ${PVE_MAJOR:-unknown}"

# ── what sort of container ───────────────────────────────────────────────────

CTID=$(pvesh get /cluster/nextid)
HOSTNAME="stundenglas"
DISK=12
CORES=2
RAM=4096
BRIDGE="vmbr0"
PUBLIC_URL=""
TUNNEL_TOKEN=""

header
if whiptail --title "stundenglas" --yesno \
    "Make a container with the usual settings?\n\n  ID        ${CTID}\n  Hostname  ${HOSTNAME}\n  Disk      ${DISK} GB\n  Cores     ${CORES}\n  Memory    ${RAM} MB\n  Bridge    ${BRIDGE}\n\nThe disk and memory are sized for compiling; the service itself idles \
near fifty megabytes.\n\nChoose No to set each of them yourself." 22 74; then
    msg "using the usual settings"
else
    ask() {
        whiptail --title "stundenglas" --inputbox "$1" 9 66 "$2" 3>&1 1>&2 2>&3 \
            || die "cancelled; nothing was created"
    }
    CTID=$(ask "Container ID" "$CTID")
    HOSTNAME=$(ask "Hostname" "$HOSTNAME")
    DISK=$(ask "Disk, in gigabytes (the build wants ten or so)" "$DISK")
    CORES=$(ask "Cores" "$CORES")
    RAM=$(ask "Memory, in megabytes (link-time optimisation is greedy)" "$RAM")
    BRIDGE=$(ask "Network bridge" "$BRIDGE")
fi

if pct status "$CTID" &>/dev/null; then
    die "container $CTID already exists; pick another ID"
fi

PUBLIC_URL=$(whiptail --title "stundenglas" --inputbox \
    "The address the world will reach it at.\n\nIt is written into the calendar links, and a security key is bound to \
this host for good — changing it later invalidates every key already \
enrolled, so settle it now." 14 74 "https://stundenglas.example.com" 3>&1 1>&2 2>&3) \
    || die "cancelled; nothing was created"
[[ $PUBLIC_URL == https://* ]] || warn "not an https address; browsers will refuse to keep the session cookie"

# Where cloudflared sits decides what the server may listen on. In the same
# container, the loopback suffices; on the host or elsewhere, it must be
# reachable across the bridge.
if whiptail --title "cloudflared" --yesno \
    "Install cloudflared inside this container?\n\nYes — you will be asked for a tunnel token, and the server listens on \
loopback only.\n\nNo — you already run cloudflared elsewhere (the Proxmox host, say). The \
server will listen on the bridge instead, and you point an ingress rule at \
it." 16 74; then
    TUNNEL_TOKEN=$(whiptail --title "cloudflared" --passwordbox \
        "The tunnel token, from Zero Trust › Networks › Tunnels.\n\nLeave empty to install cloudflared without connecting it." 11 74 \
        3>&1 1>&2 2>&3) || TUNNEL_TOKEN=""
    LISTEN="127.0.0.1:8080"
    WITH_CLOUDFLARED=1
else
    LISTEN="0.0.0.0:8080"
    WITH_CLOUDFLARED=0
fi

# ── storage and template ─────────────────────────────────────────────────────

pick_storage() {
    local content=$1 label=$2 found
    mapfile -t found < <(pvesm status -content "$content" | awk 'NR>1 {print $1}')
    ((${#found[@]})) || die "no storage on this host takes $content"
    if ((${#found[@]} == 1)); then
        echo "${found[0]}"
        return
    fi
    local menu=()
    for one in "${found[@]}"; do menu+=("$one" ""); done
    whiptail --title "stundenglas" --menu "Which storage for the $label?" 16 60 6 "${menu[@]}" 3>&1 1>&2 2>&3 \
        || die "cancelled; nothing was created"
}

STORAGE=$(pick_storage rootdir "container")
TEMPLATE_STORE=$(pick_storage vztmpl "templates")
ok "container on ${STORAGE}, templates on ${TEMPLATE_STORE}"

msg "looking for a Debian template"
pveam update >/dev/null 2>&1 || warn "could not refresh the template list; using what is already here"
TEMPLATE=$(pveam available -section system | awk '{print $2}' | grep -E '^debian-1[23]-standard' | sort -V | tail -1)
[[ -n $TEMPLATE ]] || die "no Debian 12 or 13 template offered by this host"

if ! pveam list "$TEMPLATE_STORE" 2>/dev/null | grep -qF "$TEMPLATE"; then
    msg "downloading ${TEMPLATE}"
    pveam download "$TEMPLATE_STORE" "$TEMPLATE" >/dev/null
fi
ok "template ${TEMPLATE}"

# ── the container ────────────────────────────────────────────────────────────

msg "creating container ${CTID}"
# nesting=1 is not decoration: the service unit asks systemd for a mount
# namespace (PrivateTmp, ProtectSystem), which an unprivileged container
# cannot grant without it, and the service would refuse to start.
pct create "$CTID" "${TEMPLATE_STORE}:vztmpl/${TEMPLATE}" \
    --hostname "$HOSTNAME" \
    --cores "$CORES" \
    --memory "$RAM" \
    --rootfs "${STORAGE}:${DISK}" \
    --net0 "name=eth0,bridge=${BRIDGE},ip=dhcp" \
    --unprivileged 1 \
    --features nesting=1 \
    --onboot 1 \
    --tags stundenglas \
    --description "stundenglas — WebUntis as a calendar feed" >/dev/null
ok "container ${CTID} created"

pct start "$CTID" >/dev/null
msg "waiting for the network"
for _ in {1..30}; do
    if pct exec "$CTID" -- getent hosts deb.debian.org &>/dev/null; then break; fi
    sleep 2
done
pct exec "$CTID" -- getent hosts deb.debian.org &>/dev/null \
    || die "the container has no working DNS; check the bridge and your DHCP"
ok "network up"

inside() { pct exec "$CTID" -- bash -c "$1"; }

msg "installing what the build needs (a few minutes)"
inside "export DEBIAN_FRONTEND=noninteractive
        apt-get update -qq
        apt-get install -y -qq curl ca-certificates git build-essential nano >/dev/null"

# Debian's rustc is older than this workspace's edition, so the toolchain comes
# from rustup rather than apt.
msg "installing the Rust toolchain"
inside "export RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo
        curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path >/dev/null 2>&1"

msg "building stundenglas-server — this is the long part"
inside "export RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:\$PATH
        rm -rf /opt/stundenglas
        git clone --depth 1 --branch '${BRANCH}' '${REPO}' /opt/stundenglas >/dev/null 2>&1
        cd /opt/stundenglas
        cargo build --release -p stundenglas-server 2>&1 | tail -5
        install -m755 target/release/stundenglas-server /usr/local/bin/stundenglas-server"
ok "built and installed"

# ── its account, its secrets, its unit ───────────────────────────────────────

msg "settling the service"
KEY=$(pct exec "$CTID" -- /usr/local/bin/stundenglas-server generate-key)

inside "id stundenglas &>/dev/null || useradd --system --no-create-home --shell /usr/sbin/nologin stundenglas
        install -d -m755 /etc/stundenglas"

# The key opens every stored WebUntis password, so the file holding it is
# readable by root alone and written without ever passing through a command line.
pct exec "$CTID" -- tee /etc/stundenglas/server.env >/dev/null <<ENV
# Filled by the installer:
STUNDENGLAS_KEY=${KEY}
PUBLIC_URL=${PUBLIC_URL}
LISTEN=${LISTEN}

# Yours to fill. The session-mode pooler, not the transaction one on 6543:
# sqlx prepares its statements, and transaction pooling breaks them.
DATABASE_URL=
SUPABASE_URL=
SUPABASE_ANON_KEY=
# Required at startup, though nothing in the code reads it:
SUPABASE_SERVICE_KEY=

# Addresses admitted at once, and allowed to admit others.
ADMIN_EMAILS=

# Only if you want writing into Google Calendar rather than being polled.
# The redirect URI is <PUBLIC_URL>/google/callback.
#GOOGLE_CLIENT_ID=
#GOOGLE_CLIENT_SECRET=
ENV
inside "chmod 600 /etc/stundenglas/server.env"

inside "install -m644 /opt/stundenglas/systemd/stundenglas-server.service \
            /etc/systemd/system/stundenglas-server.service
        systemctl daemon-reload
        systemctl enable stundenglas-server >/dev/null 2>&1"
ok "service installed, and left stopped until you fill its configuration"

if ((WITH_CLOUDFLARED)); then
    msg "installing cloudflared"
    inside "curl -fsSL -o /tmp/cloudflared.deb https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64.deb
            dpkg -i /tmp/cloudflared.deb >/dev/null 2>&1
            rm -f /tmp/cloudflared.deb"
    if [[ -n $TUNNEL_TOKEN ]]; then
        printf '%s' "$TUNNEL_TOKEN" | pct exec "$CTID" -- bash -c \
            'cloudflared service install "$(cat)" >/dev/null 2>&1'
        ok "cloudflared connected; add a public hostname pointing at http://127.0.0.1:8080"
    else
        ok "cloudflared installed but not connected"
    fi
fi

IP=$(pct exec "$CTID" -- hostname -I | awk '{print $1}')

cat <<DONE

$(ok "container ${CTID} is ready at ${IP}")

  What remains, in order:

    1. Fill the blanks in the environment file:
         pct exec ${CTID} -- nano /etc/stundenglas/server.env

    2. Point ${PUBLIC_URL#https://} at it. $(if ((WITH_CLOUDFLARED)); then
         echo "cloudflared runs inside this"
         echo "       container, so the ingress goes to http://127.0.0.1:8080"
       else
         echo "Add an ingress rule to the tunnel"
         echo "       you already run, pointing at http://${IP}:8080 — and give the"
         echo "       container a DHCP reservation, or that address will move out"
         echo "       from under the ingress one reboot from now"
       fi)

    3. On the hosted Supabase project set the site URL to ${PUBLIC_URL},
       and the WebAuthn relying party to ${PUBLIC_URL#https://}. The
       config.toml in the repository governs only a local stack.

    4. Turn Bot Fight Mode off for the zone. Calendar pollers are not
       browsers, and a challenge makes every subscription go quietly stale.

    5. Start it:
         pct exec ${CTID} -- systemctl start stundenglas-server
         pct exec ${CTID} -- journalctl -u stundenglas-server -f

  Keep a copy of STUNDENGLAS_KEY somewhere that is neither this container
  nor wherever its backups land. Without it every stored password is lost
  and everyone must enter theirs afresh.

DONE
