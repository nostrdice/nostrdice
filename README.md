# nostr-dice

## Test setup

First:

```bash
just all
```

If you run into the error `server is still in the process of starting`, try again:

```bash
just all
```

### Requirements

- `docker`.
- `docker compose`.
- [`just`](https://github.com/casey/just).
- [`nostr-tool`](https://github.com/0xtrr/nostr-tool).

### To test new changes to nostrdice

Build the crate and the docker image:

```bash
cargo build
docker build --no-cache -t nostr-dice .
```

## To test the flow

You will need a nostr client e.g. [`algia`](https://github.com/mattn/algia).
From now on we assume that you use `algia`, but other clients should work as long as you can configure the relay, nsec and NWC settings.

You will need to configure your client to use the expected relay, nsec and NWC settings.
To do so, copy [this configuration file](./roller-config.json) to `~/.config/algia/roller-config.json`.

Find your multiplier:

```
~ algia -a roller search
npub130nwn4t5x8h0h6d983lfs2x44znvqezucklurjzwtn7cv0c73cxsjemx32: note1gsc66mle93sqfj8k96qj63pkma7ume6vruywkk84jee6hwkualzsynp02d
Win 1.05x the amount you zapped if the rolled number is lower than 60541! nostr:note17fh4dpcf4n5624hynj6nge7ehmawe24djqrr00ks8z9x3w8tm6nqezwcga
```

Zap one of the latest notes:

```
algia -a roller zap --amount 50000 note1gsc66mle93sqfj8k96qj63pkma7ume6vruywkk84jee6hwkualzsynp02d
```

Wait about a minute and check if the roller's LND got paid.
One way to check is to look for outgoing payments on nostrdice's LND:

```
docker exec -it bob lncli --macaroonpath=/home/lnd/.lnd/data/chain/bitcoin/regtest/admin.macaroon --tlscertpath=/home/lnd/.lnd/tls.cert listpayments
```

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

### Generate self-signed certificate for `roller-lnurl-server-proxy`

```bash
openssl req -x509 -newkey rsa:4096 -keyout key.pem -out cert.pem -days 365 -nodes -subj "/CN=localhost" -addext "subjectAltName = DNS:localhost,DNS:roller-lnurl-server-proxy" -addext 'basicConstraints=critical,CA:FALSE' -addext 'extendedKeyUsage=serverAuth'
```
