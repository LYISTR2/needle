# Needle / 针

Rust 极简 Linux 探针：基础状态、TCP 延迟、持久化流量。**只观测，不控制。**

无远程命令、文件管理、自动更新、告警平台。面板只读、仅监听 Unix socket；agent 只主动上报。前端内嵌，无 CDN 和 Node.js 运行时。支持明暗主题及安全配色自定义。

## 从源码构建

需要 Rust stable 和 C 编译器。构建后将二进制复制到项目根目录，即可执行下面的安装命令：

```sh
git clone https://github.com/LYISTR2/needle.git
cd needle
cargo build --release --locked
cp target/release/needle ./needle
```

## 安装面板

前提：root、Linux/systemd（建议 252+）、Python 3（管理助手）、Nginx、域名和 HTTPS 证书。二进制包为 Linux x86_64 GNU，实测 Debian 13；其他环境见 [完整指南](docs/guide.md)。已有同名文件请先备份。

在解压目录执行，把域名换成自己的：

```sh
install -m 0755 needle /usr/local/bin/needle
install -m 0755 deploy/needle-admin /usr/local/sbin/needle-admin
install -d -m 0700 /etc/needle
needle init --config /etc/needle/panel.json --origin https://probe.example.com
install -m 0644 deploy/needle-panel.service /etc/systemd/system/needle-panel.service
systemctl daemon-reload
systemctl enable --now needle-panel
```

**保存初始化输出的只读密钥。** 按 `deploy/nginx.conf` 配置反代、替换域名和证书；不要覆盖现有 Nginx 主配置或重复定义 `default_server`。默认 Nginx worker 组为 `www-data`。

```sh
nginx -t && systemctl reload nginx
```

浏览器访问 HTTPS 域名，输入只读密钥。不提供 IP+端口访问模式。

## 添加节点

先在目标 VPS 用 `ip route show default` 确认网卡；以下假设 `eth0`。在面板服务器执行：

```sh
needle-admin add-node hk-01 '香港 01' eth0 /root/hk-01-agent.json
```

通过 SSH/SCP 安全传输生成的配置与本软件包。在目标 VPS 的解压目录执行：

```sh
install -m 0755 needle /usr/local/bin/needle
install -m 0755 deploy/needle-admin /usr/local/sbin/needle-admin
install -d -m 0700 /etc/needle
install -m 0600 hk-01-agent.json /etc/needle/agent.json
install -m 0644 deploy/needle-agent.service /etc/systemd/system/needle-agent.service
systemctl daemon-reload
systemctl enable --now needle-agent
```

网卡、TCP 延迟目标、采样间隔在 `/etc/needle/agent.json` 配置；修改后重启 agent。不要把桥接/veth 和外网接口重复计入流量。

## 常用操作

```sh
systemctl status needle-panel needle-agent --no-pager
journalctl -u needle-agent -n 30 --no-pager
systemctl restart needle-agent

# 目标 VPS：替换累计接收、发送总量，单位字节；原账本自动备份
systemctl stop needle-agent
needle-admin set-traffic 123456789012 98765432100
systemctl start needle-agent

# 面板服务器：删除节点并立即撤销密钥
needle-admin remove-node hk-01
```

账本位于 `/var/lib/needle-agent`；面板数据在 `/var/lib/needle-panel`。不要清空账本，也不要绕过管理助手以 root 创建 SQLite/WAL 文件。

**手动更新：**备份二进制、配置和停止服务后的完整状态目录，停服务、替换二进制、启动并检查上报。没有自动更新通道。

## 文档

- [完整指南](docs/guide.md)：反代、配置、流量口径、备份、主题、构建。
- [安全模型](SECURITY.md)：权限边界、凭据泄露影响与已知限制。
- `docs/index.html`：预生成静态文档；`docs/llms.txt`：AI 可读全文。
- `docs/themes.html`：本地主题配色预览，不依赖外部主题站。

重启后累计流量不会主动归零；但突然断电后跨系统重启可能丢失末次落盘后的流量，**不是零丢失计费系统**。延迟是 TCP 建连耗时，不是 ICMP。安全措施不是无漏洞保证，本版尚未经过独立安全审计。
