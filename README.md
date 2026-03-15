# cybernode

infrastructure for the [Bostrom](https://cyb.ai) blockchain network and related services. this repo is the central point for maintaining cyberia (cyb, cyber) infrastructure — server configurations, service definitions, monitoring, and operational runbooks.

live status: https://cybernode.ai

## quick start

| I want to... | go to |
|---|---|
| connect my wallet | [chain config](#chain-configuration) |
| use the API | [endpoints](#public-endpoints) |
| bridge tokens | [IBC](#ibc) |
| check network status | https://cybernode.ai |
| explore the app | https://cyb.ai |

## servers

7 servers hosted at Hetzner (Germany + Finland).

| server | role | type | RAM | storage | GPU |
|--------|------|------|-----|---------|-----|
| Deimos | archive node + cyberindex GraphQL | dedicated | 64 GB | 6.8 TB | GTX 1080 |
| Jupiter | RPC node + IBC relayer (Hermes) | dedicated | 64 GB | 6.9 TB | GTX 1080 |
| Io | IPFS storage + cluster | dedicated | 64 GB | 3.5 TB | — |
| Cyberproxy | nginx reverse proxy + frontend | cloud CX31 | 8 GB | 80 GB | — |
| Mimas | monitoring (Grafana + Prometheus) | cloud CX41 | 16 GB | 160 GB | — |
| Port | market data APIs | cloud CX21 | 4 GB | 40 GB | — |
| Helia-Relay | libp2p circuit relay | cloud CX32 | 8 GB | 80 GB | — |

GPU (NVIDIA GTX 1080+) is required for Bostrom consensus — the network uses GPU-accelerated PageRank for knowledge graph ranking.

## services

**Deimos** — `bostrom_arch` (full history from genesis), `cyberindex` (blockchain indexer), `hasura` (GraphQL engine), `postgres` (index DB)

**Jupiter** — `bostrom_pruned` (pruned RPC node), `space-pussy-arch` (testnet archive), Hermes IBC relayer (systemd)

**Io** — IPFS daemon (Kubo 0.22), IPFS cluster (replication), libp2p swarm bootstrap

**Cyberproxy** — nginx reverse proxy, cyb.ai static frontend, webhook CI/CD

**Mimas** — Grafana 12.3, Prometheus 2.54, blackbox exporter

**Port** — market-data API (token supply), warp-dex API (DEX data)

**Helia-Relay** — libp2p circuit relay, Caddy (auto-renewing SSL)

## public endpoints

| service | url | description |
|---------|-----|-------------|
| RPC | `https://rpc.bostrom.cybernode.ai` | Tendermint RPC (HTTP + WebSocket) |
| LCD/REST | `https://lcd.bostrom.cybernode.ai` | Cosmos SDK REST API |
| gRPC-Web | `https://grpc.bostrom.cybernode.ai` | gRPC-Web proxy for browsers |
| GraphQL | `https://index.bostrom.cybernode.ai/v1/graphql` | cyberindex GraphQL API |
| IPFS gateway | `https://gateway.ipfs.cybernode.ai` | public IPFS gateway |
| IPFS swarm | `swarm.io.cybernode.ai` | libp2p bootstrap node |
| status | `https://cybernode.ai` | public status dashboard (90-day history) |
| Grafana | `https://cybernode.ai/grafana/` | detailed metrics dashboards |
| frontend | `https://cyb.ai` | main web interface |

### API examples

```bash
# latest block height
curl -s https://rpc.bostrom.cybernode.ai/status | jq '.result.sync_info.latest_block_height'

# account balance
curl -s "https://lcd.bostrom.cybernode.ai/cosmos/bank/v1beta1/balances/bostrom1..." | jq

# staking params
curl -s https://lcd.bostrom.cybernode.ai/cosmos/staking/v1beta1/params | jq
```

```graphql
# recent cyberlinks
query {
  cyberlink(limit: 10, order_by: {height: desc}) {
    particle_from
    particle_to
    neuron
    height
  }
}
```

Cosmos SDK v0.47 endpoints: use `/cosmos/*/v1beta1/*` and `/cyber/*/v1beta1/*`. legacy REST endpoints are deprecated.

## chain configuration

| parameter | value |
|-----------|-------|
| chain ID | `bostrom` |
| chain name | Bostrom |
| native token | BOOT |
| bech32 prefix | `bostrom` |
| coin type | 118 (Cosmos) |
| block time | ~5 seconds |
| consensus | CometBFT (Tendermint) |
| max validators | 100 |
| unbonding period | 21 days |
| min gas price | 0 BOOT |

### tokens

| token | denom | decimals | purpose |
|-------|-------|----------|---------|
| BOOT | `boot` | 0 | native staking and gas |
| HYDROGEN | `hydrogen` | 0 | bandwidth resource |
| MILLIAMPERE | `milliampere` | 3 | resource (1/1000 AMPERE) |
| MILLIVOLT | `millivolt` | 3 | resource (1/1000 VOLT) |

### wallets

Bostrom is supported by Keplr, Leap, and Cosmostation. auto-suggested when visiting https://cyb.ai.

### block explorers

- https://cyb.ai/oracle
- https://mintscan.io/bostrom
- https://atomscan.com/bostrom

### faucet

no public faucet. get BOOT via Osmosis DEX, IBC bridge from Osmosis, or community Telegram.

## IBC

### active channels

| counterparty | bostrom channel | remote channel | status |
|---|---|---|---|
| Osmosis | `channel-2` | `channel-95` | active (restored Feb 2026) |
| Cosmos Hub | `channel-8` | `channel-341` | expired (proposal #1023 failed quorum) |

### supported tokens

BOOT, HYDROGEN, MILLIAMPERE, MILLIVOLT can transfer to Osmosis. OSMO and ATOM can transfer to Bostrom.

### usage

bridge via https://cyb.ai/teleport or CLI:

```bash
cyber tx ibc-transfer transfer transfer channel-2 osmo1... 1000000boot \
  --from <key> --chain-id bostrom --fees 5000boot
```

relayer: Hermes v1.13 operated by cyberia team with automatic packet relay, client updates, monitoring, and auto-restart.

## stack

| component | technology | version |
|-----------|------------|---------|
| blockchain | go-cyber (Cosmos SDK, CometBFT) | v7.0.1 (SDK 0.47.16, CMT 0.37.18) |
| smart contracts | CosmWasm | v1.5.2 |
| indexer | cyberindex + Hasura GraphQL | — |
| storage | IPFS (Kubo) | 0.22.0 |
| IBC relayer | Hermes | 1.13.2 |
| monitoring | Prometheus + Grafana | 2.54.1 / 12.3.1 |
| proxy | nginx | — |
| hosting | Hetzner | DE + FI |

## networks

| network | chain ID | status |
|---------|----------|--------|
| Bostrom | `bostrom` | mainnet |
| Space Pussy | `space-pussy-1` | experimental |

## data flow

1. users interact via [cyb.ai](https://cyb.ai)
2. frontend connects to RPC/LCD endpoints through reverse proxy
3. historical queries go to the archive node indexer (GraphQL)
4. content is fetched from IPFS gateway
5. cross-chain transfers use IBC through the Hermes relayer

## storage

| data | size | location |
|------|------|----------|
| bostrom archive | ~4.4 TB | Deimos |
| bostrom pruned | ~500 GB | Jupiter |
| cyberindex DB | ~200 GB | Deimos |
| IPFS blocks | ~1.7 TB | Io |

Deimos and Jupiter use ZFS for data integrity — automated snapshots every 12 hours with 2-day retention.

## monitoring

status dashboard at [cybernode.ai](https://cybernode.ai) — Grafana with Prometheus metrics from all servers. blackbox exporter probes HTTP/SSL endpoints. node exporter for hardware metrics.

### uptime targets

| service | target |
|---------|--------|
| RPC/LCD | 99.9% |
| GraphQL | 99.5% |
| IPFS gateway | 99% |
| cyb.ai | 99.9% |

### alerts

- infrastructure: disk, RAM, CPU, block counter stalls, ZFS health, GPU status
- service: API availability, SSL certificate expiry, IPFS responsiveness, IBC relayer wallet balance
- routing: Telegram

## security

- TLS 1.3 on all public endpoints
- DDoS protection at reverse proxy layer
- key-based SSH, 2FA for operators
- 100 active validators (CometBFT BFT consensus)
- CosmWasm contracts require governance approval for deployment
- responsible disclosure: DM @groovybear on Telegram

## community

| resource | link |
|----------|------|
| Telegram | https://t.me/bostrom_news |
| Discord | https://discord.gg/cyber |
| GitHub | https://github.com/cybercongress |
| Forum | https://commonwealth.im/bostrom |
| Twitter/X | https://x.com/cyber_devs |

## repos (cyberia-to)

- [go-cyber](https://github.com/cyberia-to/go-cyber) — blockchain node
- [cyb-ts](https://github.com/cyberia-to/cyb-ts) — web frontend
- [cyber-ts](https://github.com/cyberia-to/cyber-ts) — TypeScript client
- [soft3.js](https://github.com/cyberia-to/soft3.js) — JavaScript API
- [cyberindex](https://github.com/cyberia-to/cyberindex) — GraphQL indexer
- [cw-cyber](https://github.com/cyberia-to/cw-cyber) — CosmWasm semantic libs
- [warp](https://github.com/cyberia-to/warp) — DEX contracts
- [localbostrom](https://github.com/cyberia-to/localbostrom) — dev environment
- [celatone-frontend](https://github.com/cyberia-to/celatone-frontend) — block explorer
- [cybernet](https://github.com/cyberia-to/cybernet) — DAO network
