# Needle / 针

用 Rust 编写的轻量 Linux 服务器探针：一个二进制，包含监控面板与主动上报 agent。

v0.2.0 增加管理员账号密码、会话登录、网页节点管理和一键安装；界面参考 [monitor-probe/monitor](https://github.com/monitor-probe/monitor) 的简洁卡片与表格布局。参考项目采用 MIT 许可，版权声明见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

## 功能

- CPU、内存、根分区、运行时间、上下行速率和 TCP 建连延迟。
- 本地持久化流量账本，重启后持续累计；支持服务器本地设置历史累计流量。
- 管理员用户名／密码登录，Argon2id 密码哈希，12 小时会话；支持改密、退出和本地密码恢复。
- 网页新增、编辑、删除节点；分组、备注、到期日期、累计流量配额与排序。
- 按名称、ID、备注搜索，按分组和在线状态筛选。
- 添加节点后复制 agent 一键安装命令；一次性安装凭据 15 分钟有效。
- 浅色／深色主题、响应式节点列表、当前页面会话实时流量曲线。
- 旧只读密钥仍可查看，不能调用管理接口。

agent 仅主动 HTTPS 上报，无远程命令、文件管理和自动更新接口。面板仅监听 Unix socket，须使用域名和 HTTPS 反代。

## 一键安装面板

要求 Linux、systemd、Python 3、curl、CA 证书。当前提供 Linux x86_64 发行包；脚本也识别 aarch64，但需要对应架构的发行资产。

```sh
curl -fsSL https://raw.githubusercontent.com/LYISTR2/needle/main/install.sh -o needle-install.sh
sudo bash needle-install.sh panel --url https://monitor.example/needle
```

脚本下载固定版本 v0.2.0 的发行包和 SHA256SUMS，校验后安装。安装时提示设置管理员密码（至少 12 字节）。随后配置 HTTPS 反代；脚本不会覆盖现有 nginx 站点或代办证书。可以将已解压发行包传给 `--bundle /absolute/path/to/needle-0.2.0-linux-x86_64`，避免下载。

面板配置 `/etc/needle/panel.json`，数据库 `/var/lib/needle-panel/panel.db`，Unix socket `/run/needle-panel/http.sock`。独立域名反代示例见 [deploy/nginx.conf](deploy/nginx.conf)。子路径部署见 [docs/guide.md](docs/guide.md)。

## 接入服务器

1. 管理员登录面板，点击「添加服务器」。
2. 填写节点 ID、显示名称和目标机器的网卡名（用 `ip link` 查看，例如 `eth0`、`ens18`）。
3. 保存后复制安装命令，在目标机器执行。命令仅可使用一次，15 分钟后过期。

重新安装：点击节点的「安装」生成新命令。兑换凭据时才轮换上报密钥，旧 agent 随后失去上报权限；已有本地流量账本保留。删除节点只撤销身份，不会远程卸载进程。

编辑节点修改的是面板元数据，不会更改远程 agent 的网卡或延迟目标。配额是**累计上下行之和**的展示参考，不是月度计费；未设置的日期、配额不会显示虚构数据。

## 管理与恢复

```sh
# 重置管理员密码（交互输入，不将密码放进命令行）
sudo needle-admin set-admin admin
# 服务
sudo systemctl restart needle-panel
sudo systemctl restart needle-agent
# 本地调整累计流量，先停 agent
sudo systemctl stop needle-agent
sudo needle-admin set-traffic 111111111 222222222
sudo systemctl start needle-agent
```

改密或本地重置后，所有已登录会话撤销。管理员会话 cookie 使用 Secure、HttpOnly、SameSite=Strict，管理写入额外要求同源 Origin 和 CSRF 凭据。旧只读密钥保留独立权限，仅在页面内存保存。

## 从源码构建

需要 Rust stable、C 编译器。

```sh
git clone https://github.com/LYISTR2/needle.git
cd needle
cargo build --release --locked
```

部署模板位于 `deploy/`。完整说明见 [docs/guide.md](docs/guide.md)，安全边界见 [SECURITY.md](SECURITY.md)。发行打包脚本为 `scripts/package.py`，不打包本机配置、证据、备份和运行数据。

## 许可与交付状态

MIT。参考项目版权声明一并保留。本次 v0.2.0 按维护者要求未运行自动测试、lint 或项目验证器；既有测试文件保留，旧版本的验证结果不代表本版本。
