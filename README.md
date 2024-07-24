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

## Other useful commands

### Get blockchain info

```bash
docker exec -it polar-n1-backend1 bitcoin-cli -rpccookiefile=/home/bitcoin/.bitcoin/regtest/.cookie -rpcport=18443 getblockchaininfo
```

### Send to address

Replace `<address>` with your address

```bash
docker exec -it polar-n1-backend1 bitcoin-cli -rpccookiefile=/home/bitcoin/.bitcoin/regtest/.cookie -rpcport=18443 sendtoaddress <address> 10
```

### Get new address

```bash
docker exec -it alice lncli --macaroonpath=/home/lnd/.lnd/data/chain/bitcoin/regtest/admin.macaroon --tlscertpath=/home/lnd/.lnd/tls.cert newaddress p2wkh
```

### Open channel

Replace

- `pubkey` with the counterparty pubkey
- `counterparty` with the counterparty node address, e.g. `bob::9735`

```bash
docker exec -it alice lncli --macaroonpath=/home/lnd/.lnd/data/chain/bitcoin/regtest/admin.macaroon --tlscertpath=/home/lnd/.lnd/tls.cert openchannel --node_key <pubkey> --connect <counterparty> --local_amt 10000000 --push_amt 500000
```
