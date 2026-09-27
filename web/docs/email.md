# 使用真实邮箱发送注册验证码

Linux-Pilot 的邮件发送端已经接入 SMTP。只要有可用的邮箱服务账号、SMTP 主机与密码，就能把验证码发到真实邮箱；无需修改 Rust 代码。目前仓库没有你的 SMTP 凭据，因此不能代你完成公网邮件实发测试。

首次运行 `./web/scripts/init-env.sh` 会生成项目根目录 `.env`。将其中这些变量改为邮箱服务商提供的值：

```dotenv
SMTP_HOST=smtp.example.com
SMTP_PORT=587
SMTP_SECURITY=starttls
SMTP_FROM=Linux-Pilot <no-reply@example.com>
SMTP_USERNAME=no-reply@example.com
SMTP_PASSWORD=你的SMTP应用密码或凭据
```

`SMTP_SECURITY` 支持 `starttls`（常见于 587 端口）和 `tls`（常见于 465 端口）。请以服务商实际参数为准；`none` 仅供本地 Mailpit 使用。`SMTP_FROM` 必须是邮件服务允许使用的发件人地址。密码只放在 `.env` 或部署环境的密钥系统中，不应提交 Git。

修改后启动或重建服务：

```bash
docker compose up -d --build
```

在控制台使用一个能收信的邮箱注册，点击“发送验证码”并检查收件箱。若失败，查看 `docker compose logs backend`；服务会撤销该次验证码，用户可在修正 SMTP 后重试。生产模式还要求 HTTPS Web 地址、gRPC TLS 证书和随机口令；这些与 SMTP 配置是分别校验的。

邮箱验证码有效期为 10 分钟，单邮箱发送间隔 60 秒，每小时最多 5 次，单个验证码最多尝试 5 次。注册成功后，管理员可在“用户管理”页面分配角色。
