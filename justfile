all:
    # Start 2 LND nodes. This needs to be done first to ensure that certain directories are created ¯\_(ツ)_/¯
    just docker

    # Create channel from roller to nostrdice
    just create-channel

    # Create nostr profiles for roller and nostrdice
    just create-nostr-profiles

docker:
    docker compose up alice bob -d
    sleep 2

    # Start all the other containers
    docker compose up -d

    # Update nostrdice's CA certificates so that nostrdice can trust the roller's self-signed certificate
    docker exec -u 0 -it nostrdice update-ca-certificates

wipe:
    docker compose down -v

create-channel:
    #!/usr/bin/env bash

    # Create bitcoind wallet if it doesn't exist
    docker exec -it polar-n1-backend1 bitcoin-cli -rpccookiefile=/home/bitcoin/.bitcoin/regtest/.cookie -rpcport=18443 -regtest createwallet default > /dev/null

    set -euo pipefail

    # Get roller's on-chain address to be funded
    address=$(docker exec -it alice lncli --macaroonpath=/home/lnd/.lnd/data/chain/bitcoin/regtest/admin.macaroon --tlscertpath=/home/lnd/.lnd/tls.cert newaddress p2wkh | jq -r '.address')

    # Mine a few blocks and fund roller's on-chain address
    docker exec -it polar-n1-backend1 bitcoin-cli -rpccookiefile=/home/bitcoin/.bitcoin/regtest/.cookie -rpcport=18443 -regtest generatetoaddress 200 $address > /dev/null

    # TODO: Might not be enough on first startup. Can sometimes run into 'server is still in the process of starting'
    just wait-until-synced alice

    # Get nostrdice's LND node ID
    nodeId=$(docker exec -it bob lncli --macaroonpath=/home/lnd/.lnd/data/chain/bitcoin/regtest/admin.macaroon --tlscertpath=/home/lnd/.lnd/tls.cert getinfo | jq -r '.identity_pubkey')

    # Open channel from roller to nostrdice
    docker exec -it alice lncli --macaroonpath=/home/lnd/.lnd/data/chain/bitcoin/regtest/admin.macaroon --tlscertpath=/home/lnd/.lnd/tls.cert openchannel --node_key=$nodeId --connect bob:9735 --local_amt 1000000 --push_amt 500000

    # Get the LN channel funding transaction confirmed
    docker exec -it polar-n1-backend1 bitcoin-cli -rpccookiefile=/home/bitcoin/.bitcoin/regtest/.cookie -rpcport=18443 -regtest generatetoaddress 10 $address > /dev/null
    just wait-until-synced alice

create-nostr-profiles:
    nostr-tool -p nsec1r8q685ht0t8986l37hj7u3xtysjk840f0p3ed77wv04mwn6l20mqtjg99g -r ws://localhost:7000 update-metadata --lud16 bob@localhost
    nostr-tool -p nsec1vl029mgpspedva04g90vltkh6fvh240zqtv9k0t9af8935ke9laqsnlfe5 -r ws://localhost:7000 update-metadata --lud16 alice@roller-lnurl-server-proxy

wait-until-synced node:
    #!/usr/bin/env sh
    while ! docker exec -it {{node}} lncli --macaroonpath=/home/lnd/.lnd/data/chain/bitcoin/regtest/admin.macaroon --tlscertpath=/home/lnd/.lnd/tls.cert getinfo | jq -e '.synced_to_chain == true' > /dev/null; do
      echo "Waiting for LND to sync to chain"
      sleep 1
    done
