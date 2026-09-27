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

## Endpoints

All endpoints are WebSocket upgrades with `Authorization: Bearer <ACCESS_KEY>`.

| Path | Who | Purpose |
| --- | --- | --- |
| `/v1/host/{id}` | host | Control socket: connection announcements and pairing slots |
| `/v1/host/{id}/accept/{conn}` | host | Data socket for one announced client |
| `/v1/client/{id}` | client | Join a host's room; `{"t":"open"}` once the host picks up |
| `/v1/pair/{slot}` | client | Join the host showing a pairing code with this slot |

Host sockets also send `X-Host-Token`. The first token a host id registers
with is kept, so another machine that knows the id cannot take over the
room.
