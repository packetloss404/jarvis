# jarvis-mobile

Expo (React Native) client for Jarvis: **relay terminal** (encrypted PTY bridge to desktop), **embedded livechat** (Supabase + Web Crypto in a WebView), and **Claude Code** (claude.ai in a WebView).

Parent repo: [github.com/dyoburon/jarvis](https://github.com/dyoburon/jarvis) — see root [README.md](../README.md) and [ARCHITECTURE.md](../ARCHITECTURE.md).

## Run

```bash
cd jarvis-mobile
npm install
npx expo start
```

Then open the iOS simulator, Android emulator, or Expo Go / dev client as usual.

## Scripts

| Script | Purpose |
|--------|---------|
| `npm run lint` | ESLint (Expo flat config) |
| `npm run typecheck` | `tsc --noEmit` |
| `npm run test` | Jest (`jest-expo`) — pairing parser + mock relay |
| `npm run sync:chat-html` | Copy `jarvis-rs/assets/panels/chat/index.html` → `vendor/chat-index.from-jarvis-rs.html` for diff/review |

## Tabs

- **[ code ]** — Paste or scan pairing data, connect to the relay, drive a remote terminal. Session token is stored locally (AsyncStorage).
- **[ chat ]** — Livechat UI inlined in [lib/jarvis-chat-html.ts](lib/jarvis-chat-html.ts); connects to Supabase from inside the WebView.
- **[ claude ]** — `https://claude.ai/code` in a WebView. If sign-in is blocked, use **[browser]** to open the same URL in the system browser (`expo-web-browser`). Cookies are not automatically shared back into the WebView; use **reload** after signing in elsewhere if the product allows.

## Deep links

Scheme: `jarvis` (see [app.json](app.json)). Opening **`jarvis://pair?relay=...&session=...&dhpub=...`** queues pairing and navigates home so the code tab can connect once the terminal WebView is ready.

## Environment (optional)

See [.env.example](.env.example). Public build-time variables (no secrets):

- `EXPO_PUBLIC_DEFAULT_RELAY_URL` — shown as a hint in Settings when set.
- `EXPO_PUBLIC_SUPABASE_URL` — shown as a hint when set (livechat config still lives inside the chat bundle unless you refactor).

## EAS Build

[eas.json](eas.json) defines `development`, `preview`, and `production` profiles. Install [EAS CLI](https://docs.expo.dev/build/setup/) and run `eas build` when you are ready to ship.

## Threat model (short)

- **Relay:** After both sides join the relay session, mobile and desktop perform ECDH + AES-GCM for PTY messages (see [lib/crypto.ts](lib/crypto.ts), [lib/relay-connection.ts](lib/relay-connection.ts)).
- **Livechat:** E2E crypto runs in the chat WebView (Web Crypto), separate from the relay path.
- Do not share pairing strings with untrusted apps or screenshots.

## Chat HTML parity

Desktop chat uses `window.jarvis.ipc` for crypto; mobile uses Web Crypto. The sync script does **not** auto-merge those layers — use `vendor/chat-index.from-jarvis-rs.html` to see upstream changes and update [lib/jarvis-chat-html.ts](lib/jarvis-chat-html.ts) manually.
