#!/usr/bin/env bash
# Needle installer: Linux/systemd, HTTPS only. No automatic agent updates.
set -euo pipefail
umask 077
[[ $EUID -eq 0 ]] || { printf '请通过 sudo/root 运行。\n' >&2; exit 1; }
mode=${1:-}; [[ $# -gt 0 ]] && shift
url=''; enrollment=''; enroll_stdin=0; version='v0.2.0'; local_bundle=''
while [[ $# -gt 0 ]]; do
  case "$1" in
    --url) url=${2:?}; shift 2;;
    --enroll-stdin) enroll_stdin=1; shift;;
    --version) version=${2:?}; shift 2;;
    --bundle) local_bundle=${2:?}; shift 2;;
    *) printf '未知参数：%s\n' "$1" >&2; exit 1;;
  esac
done
[[ $mode == panel || $mode == agent ]] || { printf '用法：bash install.sh panel --url https://monitor.example/needle\n或在面板新增节点后复制 agent 安装命令。\n' >&2; exit 1; }
command -v systemctl >/dev/null
command -v python3 >/dev/null || { printf '请先安装 python3。\n' >&2; exit 1; }
command -v curl >/dev/null || { printf '请先安装 curl 和 ca-certificates。\n' >&2; exit 1; }
[[ $version =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { printf '版本无效\n' >&2; exit 1; }
python3 -c 'import sys,urllib.parse,re; u=urllib.parse.urlsplit(sys.argv[1]); assert u.scheme=="https" and u.hostname and "." in u.hostname and not u.username and not u.password and not u.query and not u.fragment; assert re.fullmatch(r"(?:/[A-Za-z0-9_-]+)*",u.path.rstrip("/")); import ipaddress
try: ipaddress.ip_address(u.hostname)
except ValueError: pass
else: raise SystemExit("需要域名，不支持 IP 地址")' "$url"
url=${url%/}
case "$(uname -m)" in x86_64) arch=x86_64;; aarch64|arm64) arch=aarch64;; *) printf '暂不支持该架构\n' >&2; exit 1;; esac
work=$(mktemp -d "${TMPDIR:-/var/tmp}/needle-install.XXXXXXXX")
trap 'rm -rf -- "$work"' EXIT
if [[ -n $local_bundle ]]; then
  bundle=$(realpath "$local_bundle")
else
  archive="needle-${version#v}-linux-$arch.tar.gz"
  base="https://github.com/LYISTR2/needle/releases/download/$version"
  curl --fail --show-error --location --proto '=https' --tlsv1.2 "$base/$archive" -o "$work/$archive"
  curl --fail --show-error --location --proto '=https' --tlsv1.2 "$base/SHA256SUMS" -o "$work/SHA256SUMS"
  (cd "$work"; python3 -c 'import hashlib,pathlib,sys; name=sys.argv[1]; pairs=[line.split() for line in pathlib.Path("SHA256SUMS").read_text().splitlines() if line.strip()]; expected=next(p[0] for p in pairs if p[1].lstrip("*")==name); assert hashlib.sha256(pathlib.Path(name).read_bytes()).hexdigest()==expected,"发行包校验失败"' "$archive")
  mkdir "$work/package"
  python3 -c 'import tarfile,sys; tarfile.open(sys.argv[1]).extractall(sys.argv[2],filter="data")' "$work/$archive" "$work/package"
  bundle="$work/package/needle-${version#v}-linux-$arch"
fi
[[ -x $bundle/needle && -f $bundle/deploy/needle-agent.service ]] || { printf '发行包不完整\n' >&2; exit 1; }
install -d -m 0700 /etc/needle
if [[ $mode == agent ]]; then
  [[ $enroll_stdin == 1 ]] || { printf '请从面板复制一次性安装命令。\n' >&2; exit 1; }
  enrollment=$(python3 -c 'import sys,re; s=sys.stdin.read().strip(); assert re.fullmatch("[a-f0-9]{64}",s); print(s)')
  # Download/build artifacts before consuming enrollment; failure leaves old agent untouched.
  printf '{"token":"%s"}' "$enrollment" | curl --fail --show-error --silent --proto '=https' --tlsv1.2 --connect-timeout 15 --max-time 30 -H 'Content-Type: application/json' --data-binary @- "$url/api/enroll" -o "$work/enrollment.json"
  unset enrollment
  python3 -c 'import json,sys,pathlib; d=json.load(open(sys.argv[1])); assert "config" in d; pathlib.Path(sys.argv[2]).write_text(json.dumps(d["config"],indent=2))' "$work/enrollment.json" "$work/agent.json"
  if [[ -f /etc/needle/agent.json ]]; then cp -a /etc/needle/agent.json "/etc/needle/agent.json.bak.$(date +%s)"; fi
  # Preserve state directory and its traffic ledger when reinstalling.
  systemctl stop needle-agent.service 2>/dev/null || true
  install -m 0600 "$work/agent.json" /etc/needle/agent.json
  install -m 0644 "$bundle/deploy/needle-agent.service" /etc/systemd/system/needle-agent.service
else
  [[ ! -f /etc/needle/panel.json ]] || { printf '面板配置已存在；为避免覆盖，安装已停止。升级请手动替换二进制。\n' >&2; exit 1; }
  getent group www-data >/dev/null || groupadd --system www-data
  origin=$(python3 -c 'import urllib.parse,sys; u=urllib.parse.urlsplit(sys.argv[1]); print(u.scheme+"://"+u.netloc)' "$url")
  "$bundle/needle" init --config /etc/needle/panel.json --origin "$origin" > /etc/needle/viewer-initialization.txt
  python3 -c 'import json,sys,urllib.parse,pathlib; p=pathlib.Path("/etc/needle/panel.json"); c=json.loads(p.read_text()); c["path_prefix"]=urllib.parse.urlsplit(sys.argv[1]).path; p.write_text(json.dumps(c,indent=2))' "$url"
  install -m 0644 "$bundle/deploy/needle-panel.service" /etc/systemd/system/needle-panel.service
  install -m 0755 "$bundle/deploy/needle-admin" /usr/local/sbin/needle-admin
fi
install -m 0755 "$bundle/needle" /usr/local/bin/needle.pending
mv -f /usr/local/bin/needle.pending /usr/local/bin/needle
systemctl daemon-reload
if [[ $mode == agent ]]; then
  systemctl enable --now needle-agent.service
  printf 'Agent 已安装并启动，流量账本保留在 /var/lib/needle-agent。\n'
else
  systemctl enable --now needle-panel.service
  /usr/local/sbin/needle-admin set-admin admin
  printf '\n面板已安装：%s/\n请按仓库 deploy/nginx.conf 配置 HTTPS 反代，Unix socket 为 /run/needle-panel/http.sock。\n' "$url"
fi
