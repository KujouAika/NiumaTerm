# NiumaTerm relay

A Cloudflare Worker that lets paired devices reach a NiumaTerm host from
outside its local network. Each user deploys their own copy to their own
Cloudflare account. The project does not run a shared relay.

The relay only forwards bytes. Every connection through it carries the same
end-to-end encrypted Noise channel as a LAN connection, and the relay never
holds a key that can read it. A compromised relay can drop or delay traffic,
but it cannot read terminals or impersonate a paired device.

## Deploy

Requires a Cloudflare account. The free plan is enough, because Durable
Objects on SQLite storage are available there.

```sh
cd relay
npx wrangler login
npx wrangler deploy
npx wrangler secret put ACCESS_KEY
```

For `ACCESS_KEY`, pick a long random string, for example the output of
`openssl rand -hex 32`. Every socket must present this key, so a stranger
who learns the relay URL cannot use your quota.

`wrangler deploy` prints the Worker URL, for example
`https://niumaterm-relay.<account>.workers.dev`.

## Configure the host

On the computer that hosts sessions, open Settings > Remote and fill in:

- **Relay URL**: the Worker URL
- **Relay access key**: the `ACCESS_KEY` value

Then click **Apply relay**. The host registers with the relay and stays
registered while hosting is on.

Devices paired after this receive the relay URL and key inside the encrypted
pairing exchange, so they need no setup. Devices paired earlier learn the
relay the next time they pair. To pair from outside the local network,
click **Copy link** next to the pairing code and paste the link into the
other computer's code field. The link carries the relay and the host's
key.

## How connections are chosen

A client tries the host's LAN addresses at once and the relay after
300 ms, or as soon as every LAN attempt has failed. The first handshake
to complete wins. On the same network the LAN usually wins.

## Local development

```sh
cd relay
echo ACCESS_KEY=local-test-key-0123456789 > .dev.vars
npx wrangler dev --local --port 8787 --ip 127.0.0.1
```

The Rust integration test pairs, then runs a shell, through a running relay
with the host's LAN listener closed:

```sh
NMT_TEST_RELAY_URL=http://127.0.0.1:8787 \
NMT_TEST_RELAY_KEY=local-test-key-0123456789 \
cargo test -p nmt_remote --lib through_the_relay -- --ignored
```

## Push notifications

The phone app can hear from a host while it is away: an agent finished,
needs approval, or asks a question. The host seals the text for the phone
and posts it to `/v1/push`; the relay signs an APNs request and forwards
the ciphertext, which only the phone can open.

APNs pushes can only be signed with a key from the Apple developer account
that signs the app, so only the relay of whoever builds the app forwards
them. That relay serves every host the app pairs with, including hosts on
other relays, which is why `/v1/push` takes no access key. It only reaches
the app named by `APNS_TOPIC`, carries ciphertext, and limits each device
token to 60 pushes a minute.

To enable it on the relay that belongs to the app's developer account:

1. In the developer account, Certificates, Identifiers & Profiles > Keys,
   create a key with Apple Push Notifications service, environment
   "Sandbox & Production", and download the `.p8` file (it downloads once).
2. Set the secrets; the file goes in through standard input:

   ```sh
   npx wrangler secret put APNS_KEY < AuthKey_XXXXXXXXXX.p8
   npx wrangler secret put APNS_KEY_ID     # the 10 characters after AuthKey_
   npx wrangler secret put APNS_TEAM_ID    # the account's team id
   npx wrangler secret put APNS_TOPIC      # the app's bundle identifier
   ```

3. Build the app with `NMT_PUSH_ENDPOINT` pointing at
   `https://<worker>/v1/push` (see mobile/ios/README.md).

Without the secrets the endpoint answers 501 and hosts send nothing. A
push with a made-up 64-digit hex token must come back `400 BadDeviceToken`
from both environments; `BadEnvironmentKeyInToken` means the key lacks an
environment. `wrangler dev` cannot reach APNs, so try pushes on a deployed
relay.

## Endpoints

All endpoints but `/v1/push` are WebSocket upgrades with
`Authorization: Bearer <ACCESS_KEY>`.

| Path | Who | Purpose |
| --- | --- | --- |
| `/v1/host/{id}` | host | Control socket: connection announcements and pairing slots |
| `/v1/host/{id}/accept/{conn}` | host | Data socket for one announced client |
| `/v1/client/{id}` | client | Join a host's room; `{"t":"open"}` once the host picks up |
| `/v1/pair/{slot}` | client | Join the host showing a pairing code with this slot |
| `/v1/push` | host | POST a sealed push for the phone app; no access key |

Host sockets also send `X-Host-Token`. The first token a host id registers
with is kept, so another machine that knows the id cannot take over the
room.
