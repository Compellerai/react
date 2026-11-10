# REACT livestream smoke path

This is the pragmatic COM-444 smoke path for livestream prep. It keeps the OBS overlay and Twitch chat relay outside the REACT render loop so the visual program feed can stay focused on audio-reactive output and the virtual stage DMX responder.

## Pieces

- REACT program feed: run REACT normally with Lisa voice/audio routed into the selected system or input capture device. Enable the virtual fixture responder with `REACT_VIRTUAL_STAGE=1` when using the COM-407/408 virtual stage path.
- OBS browser overlay: `stream-smoke/server.mjs` serves `/overlay` and `/events` for a browser source.
- Linus/status relay: POST JSON to `/overlay-message`.
- Twitch chat relay: set `TWITCH_CHANNEL=<channel>` and the server joins Twitch IRC anonymously, then emits chat messages to the same overlay event stream.

## Run locally

```bash
REACT_STREAM_SMOKE_PORT=8787 node stream-smoke/server.mjs
```

Optional Twitch chat:

```bash
TWITCH_CHANNEL=<channel> REACT_STREAM_SMOKE_PORT=8787 node stream-smoke/server.mjs
```

OBS browser source URL:

```text
http://127.0.0.1:8787/overlay?max=5&ttl=22000
```

Send a Linus/status card:

```bash
curl -sS -X POST http://127.0.0.1:8787/overlay-message \
  -H 'content-type: application/json' \
  -d '{"user":"Linus","source":"linus","kind":"status","msg":"Smoke path holds. Audio drives visuals; overlay relay is live."}'
```

Send a local chat test without Twitch:

```bash
curl -sS -X POST http://127.0.0.1:8787/chat-test \
  -H 'content-type: application/json' \
  -d '{"user":"viewer123","msg":"Can Lisa hear Twitch chat?"}'
```

Health check:

```bash
curl -sS http://127.0.0.1:8787/healthz
```

## Smoke checklist

1. Start REACT on the show machine with Lisa voice/audio routed to the selected capture source.
2. If using the virtual fixture path, set `REACT_VIRTUAL_STAGE=1` before launching REACT.
3. Start the stream smoke overlay server from the repository root.
4. Add OBS browser source `http://127.0.0.1:8787/overlay?max=5&ttl=22000`.
5. POST one `/overlay-message` from Linus and confirm it appears in OBS.
6. If the Twitch channel is known, set `TWITCH_CHANNEL` and confirm a live chat message appears in OBS. If not, use `/chat-test` and record that the only missing production input is the channel name.
7. Confirm Lisa voice/audio changes REACT visuals while overlay messages continue to render.
8. Record the exact audio route, Twitch channel, OBS source name, and any failures in the issue comment.

## Notes

- The overlay server binds to `127.0.0.1` by default for local OBS use.
- Events are appended to `stream-smoke-events.jsonl` unless `REACT_STREAM_SMOKE_LOG` is set.
- This is a smoke path, not production chat moderation or persistent show control.
