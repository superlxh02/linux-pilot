# GitHub 与 Google 登录配置

Linux-Pilot 的 OAuth 授权码交换只发生在 Web 后端。前端根据 `/api/v1/auth/providers` 的返回值决定是否启用按钮；**没有 Client ID 和 Client Secret 时不会假装登录成功**。目前仓库没有任何真实第三方凭据。

## 先准备 Web 地址

`PO_PUBLIC_URL` 必须是用户访问**Web 控制台**时看到的站点根地址，不是只展示介绍内容的 GitHub Pages 官网地址；不要带 `/login` 或其他路径。例如本地为 `http://localhost:3000`，生产为 `https://pilot.example.com`。反向代理必须将 `/api/v1/auth/oauth/...` 送到后端。Google 对非 localhost 回调要求 HTTPS；GitHub 和 Google 的回调地址都应与应用配置完全一致。

| 提供方 | 回调地址 |
| --- | --- |
| GitHub | `https://pilot.example.com/api/v1/auth/oauth/github/callback` |
| Google | `https://pilot.example.com/api/v1/auth/oauth/google/callback` |

本地联调时将表中的 `https://pilot.example.com` 换为 `http://localhost:3000`。GitHub OAuth App 的 Homepage URL 可以填 Web 控制台根地址；Google 创建 **Web application** 类型的 OAuth 客户端，并在 Authorized redirect URIs 中填上精确回调地址。

## GitHub

1. 在 GitHub Developer settings 创建 OAuth App。Authorization callback URL 填上表中的 GitHub 回调地址。
2. 取得 Client ID 和 Client Secret，放入项目根目录的 `.env`：

   ```dotenv
   GITHUB_CLIENT_ID=你的客户端ID
   GITHUB_CLIENT_SECRET=你的客户端密钥
   ```

3. 重新创建后端：`docker compose up -d --build backend`。
4. 平台请求 `read:user user:email`，只接受 GitHub API 返回的已验证邮箱，以 GitHub 数字用户 ID 作为稳定身份。

## Google

1. 在 Google Cloud 的 OAuth 配置中创建 **Web application** 客户端，设置同一站点的 Authorized redirect URI。按 Google 控制台要求配置品牌信息和测试用户。
2. 将凭据放入根目录 `.env`：

   ```dotenv
   GOOGLE_CLIENT_ID=你的客户端ID
   GOOGLE_CLIENT_SECRET=你的客户端密钥
   ```

3. 重新创建后端：`docker compose up -d --build backend`。
4. 平台请求 `openid email profile`，只接受 `email_verified=true`，以 Google `sub` 作为稳定身份。

## 验证和故障排查

```bash
curl http://localhost:3000/api/v1/auth/providers
```

返回中的 `github` 或 `google` 为 `true` 后按钮才会启用。点击按钮时，后端会生成一次性 `state` 和 PKCE S256 校验值，记录在数据库，并设置同浏览器的 HttpOnly Cookie；回调校验后由后端换取令牌。平台只创建已验证邮箱的新账号；如果邮箱已被本地账号占用，当前版本要求使用原登录方式，不会自动合并身份。

常见问题：

- `redirect_uri_mismatch`：检查提供方控制台和 `PO_PUBLIC_URL` 的协议、域名、端口及回调路径是否完全一致。
- 按钮仍显示“未配置”：两个环境变量必须同时非空，且后端需要重新创建。
- Google 测试账号被拒绝：检查 OAuth 同意屏幕的测试用户设置。
- 生产登录跳回首页但没有会话：检查 HTTPS 反向代理、Cookie 与回调是否在同一站点。

官方配置文档：[GitHub OAuth App](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/creating-an-oauth-app)、[GitHub 授权流程](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps)、[Google Web server OAuth](https://developers.google.com/identity/protocols/oauth2/web-server)。
