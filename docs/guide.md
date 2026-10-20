# Needle / 针

Rust 写的极简 Linux 服务器探针。**只观测，不控制。**

- CPU、内存、根分区、系统、运行时间、负载。
- 本地指定目标的 TCP 建连延迟（不是 ICMP ping）。
- 指定网卡的实时速率与持久化累计收发流量，可手动导入历史值。
- 私有监控面板与管理员登录，明暗主题与受限配色自定义。
- 无远程命令、终端、文件管理、自动更新、告警平台、插件执行、历史图表。

一个 Rust 二进制包含 panel 和 agent 子命令，前端资源内嵌，无 Node.js 运行时、CDN、外部字体或遥测。面板仅保存每个节点最后一份报告，不做时序数据库。

## v0.2.0：登录、管理与一键安装

默认登录方式为管理员账号密码，旧只读密钥通过「只读访问」入口使用。账号在服务器执行 `sudo needle-admin set-admin admin` 设置／恢复；密码至少 12 字节，改密会撤销全部会话。新安装器会提示设置密码。

```sh
curl -fsSL https://raw.githubusercontent.com/LYISTR2/needle/main/install.sh -o needle-install.sh
sudo bash needle-install.sh panel --url https://probe.example.com/needle
```

管理员可以网页新增／编辑／删除节点，设置分组、备注、到期日期、累计流量配额及排序，筛选在线状态。新增或点击「安装」即可生成 15 分钟有效的一次性 agent 安装命令。命令在目标服务器执行，兑换凭据后轮换报告密钥；不支持远程执行或自动更新。配额按累计上下行之和展示，不代表月度账单。

子路径部署时，在 panel.json 设置 `"path_prefix": "/needle"`，配置如下 location，原站其他路径保留：

```nginx
location = /needle { return 308 /needle/; }
location /needle/ {
    proxy_pass http://unix:/run/needle-panel/http.sock:/;
    proxy_set_header Host $http_host;
    proxy_set_header X-Forwarded-Proto $scheme;
    proxy_set_header Connection "";
    proxy_set_header X-Forwarded-For "";
    proxy_read_timeout 10s;
    client_max_body_size 16k;
    access_log off;
}
```

旧版配置不含 path_prefix 时按根路径处理；现有数据库会增加管理相关表，不清空旧节点或流量。已有安装升级前应备份二进制、配置和数据库；只替换二进制与管理助手，然后重启服务并设置管理员。发行包不再包含 evidence 目录。

本版按维护者要求不运行测试／项目验证器；下列测试命令仅供手动使用，既有测试可能仍针对旧登录布局。

## 下载包内容

`needle`：Linux x86_64 GNU release 二进制；`src/`、`web/`、`Cargo.lock`：完整源码；`deploy/`：systemd、Nginx 和本地管理助手；`docs/`：静态 HTML 和纯文本说明；`install.sh`：安装脚本。

当前构建是在 Debian 13 x86_64 上验证的 GNU 动态链接版本，需要兼容的 glibc/libgcc；**未验证 ARM、musl、Windows、macOS**。其他 Linux 请在目标系统构建。

## 构建与测试

安装 Rust stable 和 C 编译器（SQLite bundled 构建需要 C），然后：

```sh
cargo build --release --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

实际链路测试需要 Python 3、OpenSSL、curl、Nginx：

```sh
python3 tests/e2e.py
# 可选：本机 Chromium + playwright-core，及 root/systemd 沙箱实测
NEEDLE_SYSTEMD=1 NEEDLE_PLAYWRIGHT=/absolute/node_modules/playwright-core python3 tests/e2e.py
```

测试使用临时本地 CA、localhost.localdomain 和随机回环端口，不修改生产 Nginx、DNS 或系统信任库。沙箱测试创建临时任务专属 transient units 并清理。

## 安装前提

- Linux + systemd，建议 Debian 12/13、systemd 252+；本版实测 systemd 257。
- 面板有域名、有效 HTTPS 证书、Nginx。DNS 直接指向面板服务器。
- Nginx worker 组默认 `www-data`；其他发行版需同步修改 panel service 和 `needle-admin` 中的组。
- 以下命令由 root 执行，**先备份同名旧文件**，初次安装不要覆盖现有服务配置。
- 域名示例 `probe.example.com` 必须替换。不存在 IP+端口访问模式。

## 1. 面板

```sh
install -m 0755 needle /usr/local/bin/needle
install -m 0755 deploy/needle-admin /usr/local/sbin/needle-admin
install -d -m 0700 /etc/needle
needle init --config /etc/needle/panel.json --origin https://probe.example.com
```

保存终端输出的 **dashboard token** 到密码管理器，只显示一次。配置中仅存它的 SHA-256，不存明文。节点密钥不能用来解锁面板。

```sh
install -m 0644 deploy/needle-panel.service /etc/systemd/system/needle-panel.service
systemctl daemon-reload
systemctl enable --now needle-panel
systemctl status needle-panel --no-pager
```

`DynamicUser` 自动管理 `/var/lib/needle-panel`；不要手工创建、chown 或以 root 运行 `add-node` 直接操作该数据库。使用下面的管理助手保持 SQLite/WAL 所有权正确。

## 2. HTTPS 反向代理

阅读 `deploy/nginx.conf`，替换域名和证书路径，放进现有 Nginx 的 `http {}` 配置范围（通常是 `conf.d`）。**如果已经有 default_server，合并拒绝访问规则，不要重复定义，更不要直接覆盖整个 nginx.conf。**

```sh
nginx -t
systemctl reload nginx
```

关键约束：

- 反代连接 `/run/needle-panel/http.sock`，面板没有 TCP listener。
- `Host` 保留客户端域名，`X-Forwarded-Proto` 由代理覆盖为 `$scheme`，不能相信客户端原值。
- 非配置域名、IP Host、非 HTTPS、异源 Origin 被拒绝。
- 默认 TLS server 拒绝握手，HTTP IP 请求关闭连接；正常域名 HTTP 跳转 HTTPS。
- Nginx 承担公网连接数、请求速率、慢请求和请求体限制。
- 用真实域名浏览页面并输入 dashboard token。页面 shell 可以公开下载，但任何节点数据需要鉴权。

注意：域名最终仍解析到 IP。所谓禁止 IP 访问是禁止裸 IP Host/错误 SNI，不是阻止知道 IP 的人携带合法域名连接，也不是隐藏源站。

## 3. 注册与安装 agent

先在 **目标 VPS** 查看外网接口，例如：

```sh
ip route show default
```

假设是 `eth0`，在 **面板服务器** 注册：

```sh
needle-admin add-node hk-01 '香港 01' eth0 /root/hk-01-agent.json
```

注册只生成一份私密 agent 配置，不远程安装或执行任何东西。通过受保护的 SSH/SCP 把配置传到目标机器，在 **目标 VPS** 安装：

```sh
install -m 0755 needle /usr/local/bin/needle
install -m 0755 deploy/needle-admin /usr/local/sbin/needle-admin
install -d -m 0700 /etc/needle
install -m 0600 hk-01-agent.json /etc/needle/agent.json
install -m 0644 deploy/needle-agent.service /etc/systemd/system/needle-agent.service
systemctl daemon-reload
systemctl enable --now needle-agent
journalctl -u needle-agent -n 30 --no-pager
```

安全传输后删除非必要的 enrollment 配置副本。不要把 token 放在命令行参数、URL、Git、聊天或访问日志里。

agent 默认每次上报后等待 10 秒，允许 5–30 秒；上报失败不影响本地流量账本。45 秒没有成功上报就显示离线。agent 不监听网络端口，HTTPS 证书校验不可关闭、不跟随重定向、不使用环境代理。

### agent 配置字段

- `endpoint`：仅 HTTPS 域名 origin，无路径、查询参数或用户名密码。
- `path_prefix`：可选，默认空；部署在子路径时填写 `/needle` 等安全路径，不带尾部斜杠，不能包含查询或跳转地址。反代需剥离此前缀；浏览器资源和 API 使用相对路径。
- `node_id`、`token`：注册时生成的身份与独立 256-bit token。
- 前端包含当前页面会话的实时网速曲线，最多 60 个采样点；刷新后清空，不存历史数据库。
- `state_dir`：默认 `/var/lib/needle-agent`，不要丢失或随意清空。
- `interfaces`：明确列举要统计的外网接口，例如 `["eth0"]`；不统计 `lo`。不要同时统计 bridge、veth 和对应物理接口，避免重复。
- `interval_seconds`：5–30，默认 10。
- `targets`：最多 8 个本地固定的 TCP 目标，不接受面板下发。例如 `[{"name":"目标 A","address":"1.1.1.1:443"}]`。IPv6 使用 `[地址]:端口`。目标由运维自行选择，只对你有权测试的端点启用。
- `ca_certificate`：可选私有 PKI 根证书绝对路径，默认 `null`；不是关闭 TLS 校验。放在服务可读且非 `/root` 的位置。

修改配置后重启 agent。首次运行自动计入当前 boot 已有网卡流量，速率从实际采样间隔计算，不把累计流量当瞬时速率。

## 4. 导入历史流量

在目标 VPS 上，单位是 **字节**，输入值替换累计总量，不是额外相加：

```sh
systemctl stop needle-agent
needle-admin set-traffic 123456789012 98765432100
systemctl start needle-agent
```

此操作在同一 DynamicUser 身份下执行，自动备份原账本、保留防重放序号、重置当前网卡基线。运行中的 agent 有文件锁，拒绝并发修改。

### 统计边界

账本通过临时文件 + fsync + 原子 rename + 目录 fsync 持久化，使用 Linux boot ID 区分重启，接口计数回退时按重置处理。服务正常退出尝试再保存一次。

**不是计费账本或零丢失承诺**：断电/强制终止且随后重启会丢失最后一次落盘后至重启间的流量；面板网络超时和延迟采样会拉长实际采样间隔。agent 关闭期间跨越多次重启、网卡被重建但计数已超过旧基线等情况也无法完整还原。VPS 商家的统计口径可能不同。磁盘损坏/写满会报错退出，不会把坏账本静默清零。

服务重启但系统未重启时，会利用现有内核计数补算。不要复制旧账本回滚序号；面板会拒绝旧序号。若节点状态完全丢失，需要本地删除并重新注册节点、再导入有依据的历史总量。

## 5. 删除节点

在面板服务器：

```sh
needle-admin remove-node hk-01
```

立即删除最近报告并撤销该节点 token；其他节点不受影响。面板页面没有写权限。停止目标 VPS 上的 agent，避免持续失败重试。

## 6. 主题

页面自带明暗切换，只在浏览器 localStorage 保存主题选择，不保存登录 token。

在 `/etc/needle/panel.json` 的 `theme` 字段设置颜色，重启 panel 生效：

```json
{"background":"#edf2f6","surface":"#ffffff","text":"#263b4a","muted":"#627989","accent":"#287d91","line":"#d7e1e8"}
```

只允许这六个键及 `#RRGGBB` 值。自定义颜色覆盖两种模式；清空为 `{}` 恢复内置明暗配色。**不支持上传 JS/任意 CSS/HTML**，避免主题成为代码执行或凭据窃取通道。`docs/themes.html` 是随包附带的静态配色预览，不声称已有线上主题站。

## 7. 备份、手动更新和卸载

- 备份 panel 配置、dashboard token（密码管理器）、agent 配置与账本。
- 最简单的一致性备份方式：先停止对应服务，再备份整个 StateDirectory（包含数据库/WAL）并保留权限。不要只复制正在写入的单个 `.db`。
- 手动更新：确认来源与校验值 → 备份旧二进制/配置/状态 → 停服务 → 替换二进制 → 启动 → 检查日志与最新报告。没有自动更新通道。
- 卸载：先停用对应 service，删除本项目安装的 unit 和二进制并 daemon-reload；是否删除状态由你决定。不要自动删账本。

## 安全边界与限制

详见 `SECURITY.md`。Rust、少功能、测试和 systemd 沙箱可以降低风险，**不意味着没有漏洞，也不等于做过独立安全审计**。

默认最多 256 节点，每份报告最多 16 KiB。使用单线程异步 runtime；SQLite 操作是同步的，目标是小规模私有部署，不承诺大规模吞吐。无历史趋势、告警、远程管理、自动发现、用户系统或公开分享模式。

## AI 部署提示词

> 请先阅读本项目 README、SECURITY.md 和 deploy 文件。先确认目标机器、域名、证书、Nginx 用户组和网卡名，备份受影响文件；只安装本项目服务，不覆盖现有 Nginx 主配置，不开启 TCP 后端，不关闭 TLS 校验，不放宽沙箱来绕过报错。用 DynamicUser 管理助手注册节点与修改流量。完成后实际验证 HTTPS、401/403、agent 上报、账本重启续算和 service 限制；不得展示或上传 token。部署前确认授权范围。

文档 `docs/index.html` 为预生成静态 HTML，`docs/llms.txt` 为纯文本，不依赖 JavaScript。仓库中不会打包任何真实节点凭据。
