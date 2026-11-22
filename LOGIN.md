# Provider logins

Open `:login` in Magi. The centered float uses the same pane as `:cost` and the model card.

- Select a provider with the arrow keys or mouse; press **Enter** to manage it.
- **Add API key** / **Replace saved API key** opens masked input. Paste the key, then press **Enter**. **Ctrl+U** clears it; **Esc** cancels.
- For a configured OAuth provider, start browser sign-in, press **o** to open the browser, or **c** to copy its link. Finish in the browser. **Esc** cancels the waiting process; the callback expires after five minutes.
- **GitHub Copilot** has built-in device sign-in: select it, choose **Open browser to sign in**, press **o**, and approve the displayed code. **c** copies the code; **u** copies the link. **Esc** cancels. The code expires within fifteen minutes.
- **Remove saved login** asks for confirmation, defaulting to **Keep login**.
- **r** refreshes the list. After login, use `:model` to select the model you want, for example `:model mistral/devstral-latest`.

One saved credential is supported per provider identity. Adding another replaces that provider's saved credential. An environment key takes precedence over a saved key; removal does not unset environment variables or revoke credentials at the vendor.

## Ownership and storage

Magi only supplies the UI and forwards authenticated control requests to its configured model-role executable. **Melchior owns login, refresh, credential resolution and storage.** Casper's tools/MCP and Balthasar's memory are unchanged.

Melchior saves credentials in `$XDG_DATA_HOME/magi/credentials.json` (normally `~/.local/share/magi/credentials.json`). This retains the existing OAuth-store location. Files are private, mode `0600`, and writes are locked and atomic. **Credentials are not encrypted at rest.** Saved credentials are bound to the provider's authentication and endpoint configuration; changed configuration requires signing in again. Older OAuth entries retain their legacy compatibility.

Keys are sent over the private controller connection and the Melchior child's stdin, never command-line arguments. The masked field is separate from the prompt and its history. Auth replies are delivered only to the requesting UI, not the transcript, model context, timeline, observer connections or snapshots. Cancelled/disconnected authentication is not automatically retried; reopen `:login` to check the saved state if a connection fails during a write.

Both executables must include this feature. An older Melchior reports authentication unavailable rather than falling back to another credential implementation. The selected executable follows the usual `magi.melchior` / PATH configuration.

Local `providers.lua` declarations override the bundled catalog. If your own `github-copilot` declaration still uses `auth = { kind = "api-key", ... }`, change that declaration to `auth = { kind = "github-copilot" }` to expose device sign-in. The implementation does not rewrite private user configuration or install/restart either executable automatically.

## Browser-provider availability

The bundled catalog includes API-key providers, including Mistral, and **GitHub Copilot device authorization**. The float also supports Melchior's configured **authorization-code + PKCE** OAuth providers. Other subscription-provider sign-ins still need their own integrations; this is not full PI authentication parity.

An OAuth provider needs its real `authorize_url`, `token_url`, public `client_id` and scopes in Melchior's `providers.lua`, plus an already supported model protocol. Its client registration must accept a loopback redirect at `http://127.0.0.1:<ephemeral-port>`; fixed callback ports/paths and remote-browser forwarding are not implemented. Non-loopback OAuth endpoints must use HTTPS. Providers without the required configuration are displayed as unavailable. Credentials belonging to a removed provider remain visible so they can still be deleted.

## GitHub Copilot

After approval, Melchior exchanges the GitHub OAuth credential for a short-lived Copilot token, selects the subscription endpoint, and intersects the local catalog with the account's available models. It renews expired Copilot tokens under the existing cross-process credential lock. Listing models never starts login or refresh traffic. Account model access is refreshed during sign-in/token renewal; sign in again after changing model policies on GitHub. Models requiring policy activation are not automatically enabled.

Use `:model` to choose an available `github-copilot/…` entry. The bundled selection adds fourteen fully mapped PI 1.6.0 candidates across Anthropic Messages, Chat Completions and Responses, retaining the legacy Sonnet 4.5 entry. This is **not every Copilot model**: GPT candidates with unresolved compatibility fields, additional newer Claude models and the System One classifier remain outside the activated selection. A successful login cannot grant access beyond the account's subscription or organization policies.

For a headless terminal, `melchior auth login github-copilot` prints the URL and code; approve them on another device. No loopback callback is required. `melchior auth logout github-copilot` removes the local login. The environment override `COPILOT_GITHUB_TOKEN` accepts an already-minted Copilot bearer token, takes precedence, and is not renewed or account-filtered. A generic `GITHUB_TOKEN` / `GH_TOKEN` is not a Copilot credential. Clear the override to use a saved browser login.

The built-in registration follows [PI's pinned public-client integration](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/auth/oauth/github-copilot.ts) and [GitHub's device flow](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#device-flow). Custom GitHub registrations returning expiring OAuth refresh tokens are rejected, rather than silently saving credentials that cannot be renewed. GitHub Enterprise Server custom-domain login is not a bundled preset. Local tests do not establish acceptance by the real service.

## Verification

Run the native recipes, in order:

```sh
cd ../melchior
oslo make test-login
oslo make test-copilot
cd ../magi
MELCHIOR_TEST_BINARY="$(realpath ../melchior/target/debug/melchior)" oslo make test-login
```

The fixtures use synthetic credentials and local HTTP endpoints. Set `MAGI_LOGIN_PREVIEW` to an output directory to save renderings from the actual TUI test backend at 80×24 and 160×28.
