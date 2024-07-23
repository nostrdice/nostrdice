# nostr-dice

## Test setup

First

```bash
docker-compose up alice bob -d
```

wait a second or two, then start the rest.

```bash
docker-compose up -d
```

The reason for this is that LND might not start up fast enough and messes with nostr-wallet-connect-lnd.
