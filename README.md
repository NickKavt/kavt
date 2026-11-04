# Budget

A self-hosted family budget. Install it with Docker, open it in a browser and add it to your phone's home screen; it then opens like an app. Your data stays in your own database. It can also open inside Telegram as a Mini App, if you want that.

- Expenses and income by category and subcategory, in several currencies. Any operation can be changed or deleted later, from its category, its account or a report.
- Accounts: cards and cash, each in its own currency, shared or someone's own, with a starting balance. Transfers between accounts, also between currencies at that day's rate.
- An amount field that adds things up: type `120+45` and `165` is saved.
- Reports by day, week, month, year or any period.
- Savings accounts with a goal, paid into and out of by transfers from any account.
- Interest: a flexible account paid monthly or a term deposit paid at maturity. Interest is counted daily and tax is withheld once at payout, as a bank does (in lira the stopaj rate is suggested by the term). A term deposit either renews or, at its end, goes back with its interest to the account you chose; then you can open a new term. Rate changes apply from their day.
- Recurring operations and transfers (rent, subscriptions, salary, a monthly deposit): weekly, monthly or yearly, added on their day.
- CSV export of everything (accounts, starting balances, operations, transfers, interest), and CSV import from that export, a bank, Excel, Firefly III, Actual Budget or ezBookkeeping: you pick the columns, missing categories are created, and importing the same file twice adds nothing.
- Exchange rates for the budget's main currency, on each operation's own day, from the Central Bank of Türkiye, the Bank of Russia, the ECB or currency-api; or off. Only then does the server contact the chosen source.
- One budget for the family: the owner invites people with a one-time link, as members or admins.
- Members and admins, plus sessions you can end from the app.
- Interface in English, Russian and Turkish.
- Phone layout, plus a side menu and two columns on wide screens.

The server is written in Rust (axum, sqlx) and stores its data in PostgreSQL. The interface is plain ES modules with Preact and has no build step.

## What you need

- A computer that is always on, with Docker and Docker Compose: a home server, a Raspberry Pi (images are built for amd64 and arm64) or a small VPS.
- To add it to a phone's home screen as an app, an HTTPS address. Phones install web apps only over HTTPS. Any of these works:
  - [Tailscale](https://tailscale.com/kb/1312/serve) (`tailscale serve`), which gives a private HTTPS address on your own devices;
  - a domain behind [Caddy](https://caddyserver.com/), which gets the certificate for you;
  - [Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/), which needs no open ports.

  Without HTTPS the budget still works in a browser on your home network, for example at `http://192.168.1.10:8009`. It just opens as a browser tab, not as an app.

## Install

```sh
mkdir budget && cd budget
curl -fsSLO https://raw.githubusercontent.com/NickKavt/kavt/main/compose.yaml
curl -fsSL https://raw.githubusercontent.com/NickKavt/kavt/main/.env.example -o .env
```

In `.env`, set `POSTGRES_PASSWORD`: letters and digits only, for example the output of `openssl rand -hex 24`. Everything else can stay as it is.

Start it:

```sh
docker compose up -d
docker compose ps        # app shows "healthy" when it's ready
```

The app listens on `127.0.0.1:8009`, so only that computer can reach it. Then pick one:
- **HTTPS (recommended):** point a reverse proxy or tunnel at it. With Caddy, a `Caddyfile` like this is enough:
  ```
  budget.example.com {
      reverse_proxy 127.0.0.1:8009
  }
  ```
  With Tailscale: `tailscale serve --bg 8009`.
- **Home network only:** set `BUDGET_BIND=0.0.0.0` in `.env`, run `docker compose up -d` again, and open `http://<this computer's address>:8009`.

### First start

Open the address. The first page asks you to create the owner: your name, a login and a password. Do this right after installing: until then, anyone who opens the address could become the owner.

On a phone, add the app to the home screen:
- **iPhone (Safari):** Share → Add to Home Screen.
- **Android (Chrome):** ⋮ → Install app (or Add to Home screen).

### Invite people

Open **Manage → People and roles → Create a link** and send the link in any messenger. Whoever opens it picks a login and a password and joins your budget, as a member or an admin, whichever you chose. A link works once and for seven days. Unused links are listed there, and you can revoke them.

Anyone with a password can turn on **two-step sign-in** in Settings: after the password, the app asks for a code from an authenticator app (Google Authenticator, 2FAS, Bitwarden, Passwords on iPhone and others). Ten one-time recovery codes are shown once, for when the phone isn't at hand.

Passwords are stored as Argon2id hashes. A sign-in lasts 30 days on a device. Changing your password signs out your other devices. Repeated wrong passwords are slowed down.

### Telegram (optional)

The same budget can also open inside Telegram:
1. Create a bot with [@BotFather](https://t.me/BotFather) and put its token in `.env` as `TELEGRAM_BOT_TOKEN`. Restart with `docker compose up -d`.
2. In @BotFather: `/mybots` → your bot → **Bot Settings → Configure Mini App → Enable Mini App**, and enter your HTTPS address.

Inside Telegram people sign in with Telegram, without a password. In **People and roles** the owner can then also add someone by @username or Telegram ID. A person who signed in with a password and the same person in Telegram are two separate accounts.

### Locked out

On the server, in the folder with `compose.yaml`:

```sh
docker compose exec app /app/budget-server reset-password <login>   # prints a new password
docker compose exec app /app/budget-server reset-2fa <login>        # turns two-step sign-in off
```

## Update

```sh
docker compose pull && docker compose up -d
```

The database is migrated on start: each file in `migrations/` is applied once, in order, and recorded in the `schema_migrations` table. Back up before a big update. A database migrated by a newer version refuses to start with an older one, so going back means restoring the backup.

## Back up

```sh
docker compose exec -T db pg_dump -U budget -Fc budget > budget-$(date +%F).dump
```

To restore into a fresh install, first run `docker compose up -d db`, then:

```sh
docker compose exec -T db pg_restore -U budget -d budget --clean --if-exists < budget-2026-11-03.dump
docker compose up -d
```

## All settings

| Variable | Default | Meaning |
|---|---|---|
| `POSTGRES_PASSWORD` | (none) | Database password, letters and digits |
| `DATABASE_URL` | set by `compose.yaml` | PostgreSQL address |
| `BUDGET_BIND` | `127.0.0.1` | `0.0.0.0` opens the app to your home network |
| `BUDGET_SIGNUP` | `invite` | `invite`: people join with a link. `open`: anyone can create a budget of their own on the sign-in page |
| `BUDGET_SESSION_COOKIE_SECURE` | auto | HTTPS-only session cookie. Auto: when the proxy says the request came over HTTPS |
| `BUDGET_SESSION_TTL_SECONDS` | `2592000` | How long a sign-in lasts (30 days) |
| `TELEGRAM_BOT_TOKEN` | empty | Turns on opening the budget inside Telegram |
| `OWNER_TELEGRAM_ID` | empty | Telegram only: who becomes the owner if the owner signs in with Telegram first |
| `BUDGET_MAX_ACTIVE_SESSIONS_PER_USER` | `5` | Older sessions are ended beyond this |
| `TELEGRAM_INIT_DATA_TTL_SECONDS` | `86400` | Maximum age of Telegram's sign-in data |
| `BUDGET_PORT` | `8009` | Port on 127.0.0.1 that `compose.yaml` publishes |
| `RUST_LOG` | `info` | Log level |
| `BUDGET_DONATE_TON` | the author's | TON/USDT address in Settings → Support the author; empty hides it |
| `BUDGET_DONATE_GITHUB` | the author's | GitHub login for GitHub Sponsors; empty hides it |
| `BUDGET_DONATE_URL` | the author's | Any other https link (e.g. Tribute); empty hides it |
| `BUDGET_SOURCE_URL` | this repository | Settings → Source code; a modified copy must point it at its own source |

## Build from source

```sh
cargo build --release
DATABASE_URL=postgresql://user:pass@localhost/budget TELEGRAM_BOT_TOKEN=... ./target/release/budget-server
```

Or run `docker compose up -d --build` to build the image locally.

`node tools/check-i18n.mjs` checks that every text has a translation in each language.

| Path | Contents |
|---|---|
| `src/main.rs` | Routes, handlers, background cleanup of expired sessions |
| `src/local.rs` | Sign-in with a password, first-run setup, invitation links |
| `src/auth.rs` | Telegram sign-in check and sessions |
| `src/savings.rs` | Savings accounts: daily interest, payments, tax |
| `src/rates.rs` | Exchange rates from the three sources |
| `src/recurring.rs` | Recurring operations |
| `src/import.rs` | CSV export and import |
| `src/totp.rs` | Two-step sign-in: codes from an authenticator app, recovery codes |
| `src/migrate.rs` | Applies `migrations/` in order |
| `src/db.rs` | Queries: categories, totals by currency, month summary with carry-over, savings |
| `src/limit.rs` | Rate limits |
| `migrations/` | Schema changes, one numbered file each, applied once in order on start (`src/migrate.rs`) |
| `app/static/v2` | The interface: `js/screens/` for screens, `js/locales/` for texts |

## Support the project

The app is free. If it's useful to you, you can support its development:

- **GitHub Sponsors:** the "Sponsor" button at the top of this page.
- **TON or USDT on the TON network**, e.g. from Telegram Wallet: `UQDXnKJoI-zeLtZzY64B4Htk9lXPKDdd6GH32hWECSpV66Mn`

The same options are in the app under Settings → Support the author. If you run your own copy, you can change or hide them with the `BUDGET_DONATE_*` settings below.

## License

[GNU Affero General Public License v3.0](LICENSE) or later.

You may run, study, change and share this program. If you change it and let other people use the changed version over a network, you must offer them its source under the same license. Settings → Source code points to it; for a modified copy, set `BUDGET_SOURCE_URL` to where its source lives.

`app/static/v2/vendor/preact-htm.js` bundles [Preact](https://preactjs.com/) (MIT, [`LICENSE-preact`](app/static/v2/vendor/LICENSE-preact)) and [htm](https://github.com/developit/htm) (Apache-2.0, [`LICENSE-htm`](app/static/v2/vendor/LICENSE-htm)). Both licenses are compatible with the AGPL.
