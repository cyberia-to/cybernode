# cybernode

infrastructure for the [Bostrom](https://cyb.ai) blockchain network and related services. this repo is the central point for maintaining cyberia (cyb, cyber) infrastructure — server configurations, service definitions, monitoring, and operational runbooks.

## servers

| server | role | specs | location |
|--------|------|-------|----------|
| Deimos | archive node + cyberindex GraphQL | GTX 1080, 62GB RAM, 6.8TB | Falkenstein, DE |
| Jupiter | RPC node + IBC relayer (Hermes) | GTX 1080, 62GB RAM, 6.9TB | Falkenstein, DE |
| Io | IPFS storage + cluster | 62GB RAM, 3.5TB | Falkenstein, DE |
| Cyberproxy | nginx reverse proxy + frontend | CX31 | Nuremberg, DE |
| Mimas | monitoring (Grafana + Prometheus) | CX41 | Falkenstein, DE |
| Port | market data APIs | CX21 | Falkenstein, DE |
| Helia-Relay | libp2p circuit relay | CX32 | Helsinki, FI |

## services

**Deimos** — full blockchain history from genesis, cyberindex indexer, Hasura GraphQL engine, PostgreSQL

**Jupiter** — pruned RPC node, Hermes IBC relayer, space-pussy testnet archive

**Io** — IPFS daemon (Kubo), IPFS cluster replication, libp2p swarm bootstrap

**Cyberproxy** — nginx reverse proxy, cyb.ai static frontend, webhook CI/CD

**Mimas** — Grafana, Prometheus, blackbox exporter

**Port** — market-data API (token supply), warp-dex API (DEX data)

**Helia-Relay** — libp2p circuit relay, Caddy with auto-renewing SSL

## public endpoints

| service | url |
|---------|-----|
| RPC | `https://rpc.bostrom.cybernode.ai` |
| LCD/REST | `https://lcd.bostrom.cybernode.ai` |
| gRPC-Web | `https://grpc.bostrom.cybernode.ai` |
| GraphQL | `https://index.bostrom.cybernode.ai/v1/graphql` |
| IPFS gateway | `https://gateway.ipfs.cybernode.ai` |
| status dashboard | `https://cybernode.ai` |
| Grafana | `https://cybernode.ai/grafana/` |
| frontend | `https://cyb.ai` |

## stack

- **blockchain**: go-cyber (Cosmos SDK v0.47, CometBFT v0.37)
- **smart contracts**: CosmWasm v1.5
- **indexer**: cyberindex + Hasura GraphQL
- **storage**: IPFS (Kubo 0.22)
- **IBC**: Hermes v1.13
- **monitoring**: Prometheus + Grafana
- **proxy**: nginx
- **hosting**: Hetzner (DE + FI)

## data flow

1. users interact via [cyb.ai](https://cyb.ai)
2. frontend connects to RPC/LCD endpoints through reverse proxy
3. historical queries go to the archive node indexer
4. content is fetched from IPFS gateway
5. cross-chain transfers use IBC through the relayer

## redundancy

| service | strategy |
|---------|----------|
| blockchain data | ZFS snapshots (12h intervals, 2-day retention) |
| IPFS content | cluster replication |
| RPC endpoints | multiple nodes behind load balancer |
| IBC relaying | automatic restart on failure |

## monitoring

status dashboard at [cybernode.ai](https://cybernode.ai) — Grafana with Prometheus metrics from all servers. blackbox exporter probes HTTP/SSL endpoints.

uptime targets: RPC/LCD 99.9%, GraphQL 99.5%, IPFS gateway 99%.
