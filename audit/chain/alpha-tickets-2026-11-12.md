---
tags: cybernode, audit, chain, mining, rewards, pussy, alpha
crystal-type: report
crystal-domain: cyber
status: measured
date: 2026-11-12
---

# alpha-tickets — the alpha (pussy-rc) on ticket proof of work, two hosts, 24 h (P15d receipt)

The alpha rehearsal network `pussy-rc` restarted from block 0 on the settlement ticket as the
proof of work (cybernode#18, soft3#682, tru#46) on 2026-11-12 16:29 UTC, on its two production
hosts: edge-b (the public, dialable node and a 2-worker miner) and agent-a (the dial-out peer and a
4-worker miner). The grind chain it replaces (32,519 blocks, hash proof of work, since 2026-11-01)
is kept beside it on both hosts; the alpha is rehearsal data, reset at real genesis. The 24 h
result (§ the 24 h) is appended from the watcher's file by whoever closes the rehearsal.

## revisions and the binary

- soft3 `feat/ticket-pow` `8dd579b` (the facade, soft3#682 head) with its path siblings
  cybernode `feat/ticket-pow` `ad13862` (cybernode#18 head, R18-17) and tru
  `feat/coalition-marginal` `3cd4e45` (tru#46 head); the other siblings as pinned in soft3
  `release/pussy-stand.toml` (hemera 8559b52, lens d6dcc3f, bbg 2d7873c, cybergraph fe115b3,
  foculus 0551e3b, mudra 33f8029, radio b74133c, …).
- built on build-a, `/srv/build/scratch/alpha-tickets/` (`cargo build --release --locked --bin
  soft3` in `soft3/crate`, rustc 1.95.0, sccache), 2026-11-12 16:21 UTC; `soft3 0.11.0`,
  x86_64-unknown-linux-gnu, 31,286,160 bytes, sha256
  `1bed02c956d022dcfca3a9f2ed25ce4dda7dfc51cd6c8af0a1a92c8a93d0e762`, installed as
  `soft3-8dd579b` beside the earlier binaries on both hosts (`bin/soft3 → soft3-8dd579b`).
- glibc floor (the P3a lesson, gate `linux-glibc-floor`, floor 2.35): `objdump -T` highest
  `GLIBC_2.38` — one symbol, `__isoc23_sscanf`, from aws-lc-sys (cyber-radio's TLS) compiled on
  build-a's glibc 2.41 headers (`_GNU_SOURCE` selects the C23 `strto*`/`scanf` family since
  glibc 2.38; `CFLAGS=-std=gnu17` does not change it). **The gate would be red.** Both alpha hosts
  are Debian 13 (glibc 2.41, `ldd --version` on edge-b and agent-a), so the binary runs there,
  and the dev chain's binary of `audit/chain/ticket-pow-2026-11.md` ran under the same floor.
  A release binary for a 2.35 host needs the train's ubuntu-22.04 leg or a 2.35 build root on
  build-a (apparatus `servers/build-a/README.md` names that step); build-a has no container
  runtime. Not hidden, not fixed by hand.
  Fixed 17:03 UTC the same day: the same sources rebuilt on build-a in a plain Ubuntu 22.04
  chroot (glibc 2.35, rustc 1.95.0, GNU ld; apparatus `servers/build-a/jammy.sh`, the train's
  x86_64 environment) — `objdump -T` and the gate's own parser read highest `GLIBC_2.34`,
  floor 2.35 holds; 29,381,064 bytes, sha256
  `c528ea604d4b39057686748c2fa366f064cad53e8966e129db938b0887c157cd`; facade tests 2 passed,
  0 failed; placed beside the others as `bin/soft3-8dd579b-floor` on edge-b and agent-a for the
  next restart, the alpha not restarted (`bin/soft3 → soft3-8dd579b` stays for the 24 h).
- the economy's engine id in `/economy` reads `tru-epoch-e660d1e`: a constant string in
  `rs/src/chain/economy/focus.rs`, not the built tru revision (3cd4e45).

## genesis

`soft3 genesis build --chain pussy-rc --snapshot /root/cyber/space-pussy/snapshot --out
genesis/pussy-rc.tade --export genesis/export --time 1793523600` on build-a (2 s), equal to the
pinned identity in soft3 `conformance/chain-genesis.toml` `[pussy-rc]` on the branch:

| | |
|--|--|
| particle | `c1950dee724b600bafed258c7932c0d62248b87d0d5f59f3bcef8cb4760be143` |
| file sha256 | `2923fcd3a76abcf3b1906c5830c39556dc08cde2707f017dad7854d070fc6388` (3,249,660 bytes) |
| block 0 | `ff4b56ea65f8b81f86c8e58b7d3c84d1fe4e48676358efe720fa21f8ebf213b3` (the 200-byte header, zero sample) |
| time | 1793523600 (2026-11-01T09:00:00Z, the stand's genesis time, unchanged) |
| bits | `GENESIS_BITS` 0x20020000 (2^249); `W_TICKET` 64; `V_BUDGET` 6; `L` 240, `D` 120 — first settlement at height 600 |
| state root | `d48fee478584d248123c63e2ee6c56224f159c7dadbc625f4261f6af49130371` (26,805 axons, 547 neurons, 1,385 balances) |

The same file is on both hosts (`sha256sum` before start); the export TSVs feed the explorer
indexer (§ explorer).

## hosts and layout

| | edge-b (213.58.173.242) | agent-a (behind NAT) |
|--|--|--|
| node | runit `soft3-pussy-rc` (`/srv/edge/sv/soft3-pussy-rc/run`, unchanged): `bin/soft3 node --chain pussy-rc --home /var/lib/soft3/pussy-rc --bind 127.0.0.1:7783 --radio 0.0.0.0:7793 --peers /etc/soft3/pussy-rc.peers.toml --moniker edge-a-rc` | runit `soft3-pussy-rc` (`/srv/forge/sv/soft3-pussy-rc/run`, `--log-net` added; the backup `run.bak-20261112-tickets`): home `/srv/forge/claude/chain/state/pussy-rc`, bind 127.0.0.1:7783, radio 7793, peers → edge-b `54471c98…` at `213.58.173.242:7793` |
| endpoint | `54471c984e8949e006b3119c93291250930b743b1f200d69d39711af8d99ba64` (radio.key kept) | `e55304a6dffd67135878182a4cceabc5c55d81c918cf9726e786e057777fc638` (radio.key kept) |
| miner | runit `soft3-mine-pussy-rc`: `bin/soft3 mine --node http://127.0.0.1:7783 --chain pussy-rc --address 1cbb239f…e711 --workers 2` (the old `run` kept as `run.bak-20261112-tickets`; the rehearsal key `/var/lib/soft3/pussy-rc-miner.seed` unchanged) | runit `soft3-mine-pussy-rc` (new, `/srv/forge/sv/soft3-mine-pussy-rc/run`): `bin/soft3 mine … --address 523c2195…ca06 --workers 4` — the node's own neuron (apparatus `nodes.toml`), rehearsal data |
| binary | `/home/cyber/soft3-chain/bin/soft3-8dd579b`, `cybernode-check` beside it | `/srv/forge/claude/chain/bin/soft3-8dd579b`, `soft3.sha256`, `cybernode-check` |
| grind chain kept | `/var/lib/soft3/pussy-rc.grind-20261112` (height 32,519, tip `e5ac86dc…0000`) | `/srv/forge/claude/chain/state/pussy-rc.grind-20261112` (same tip) |
| cpu | 32 cores shared with the edge's services; the miner's 2 workers hold ~0.23 tickets/s | 32 cores, loaded (load ~13: the P15b dev chains under `chain-dev/` with 2×12 + 2×2 workers stay); 4 workers hold ~0.06–0.1 tickets/s |

The deploy README's `agent-a/start.sh` (tmux) is not used: agent-a runs the alpha under runit
since 2026-11-03 (apparatus `servers/agent-a/README.md` § never die), so the miner joined it as a
second runit service. Nothing else on either host was touched.

## the restart (commands, 2026-11-12 UTC)

1. 16:21 build-a: `./cargo.sh soft3/crate build --release --locked --bin soft3`; `sha256sum`,
   `objdump -T … | grep -o 'GLIBC_[0-9.]*'`; `soft3 genesis build …` (§ genesis).
2. 16:29:03 edge-b: `sv down soft3-mine-pussy-rc; sv down soft3-pussy-rc`; `mv /var/lib/soft3/pussy-rc
   /var/lib/soft3/pussy-rc.grind-20261112`; new home with `radio.key` copied back and
   `genesis.tade` installed (640 cyber); `install soft3-8dd579b`, `ln -sfn soft3-8dd579b bin/soft3`;
   the miner's `run` replaced (`--workers 2`, no `--backend`); `sv up soft3-pussy-rc` → `/status`
   height 0, genesis `c1950dee…`, tip `ff4b56ea…`; 16:30 `sv up soft3-mine-pussy-rc`.
3. 16:30 edge-b: `cybernode-check pussy-rc` — clock, disk, env, peers, radio, health, status (0.01 s),
   progress OK; `unit: cybernode@pussy-rc is not active` is the systemd probe on a runit host (exit 2 by
   that row only).
4. 16:31:00 agent-a: `sv down soft3-pussy-rc`; state moved to `pussy-rc.grind-20261112`; new home,
   `radio.key`, `genesis.tade`, binary; `sv up soft3-pussy-rc`; the miner service created and up.
5. 16:34 the pair: agent-a's dials were refused `banned (code 2)` — edge-b had banned agent-a's
   endpoint when the still-running grind node offered the old chain during the two minutes between
   the restarts (`BAN_SECS` 3600, in memory). `sv restart soft3-pussy-rc` on edge-b cleared it;
   16:34:35 `/peers` on edge-b lists agent-a inbound at block 0, same tip `ff4b56ea…`.
6. 16:32 explorer: `sv down explorer-index`; `mv /srv/edge/data/explorer /srv/edge/data/explorer.grind-20261112`;
   fresh data dir with the new genesis export (`genesis/*.tsv`, `SOURCE`) and the geo file copied;
   `sv up explorer-index` → `pussy-rc at height 0`, walking. Lesson: stop the indexer **before** the
   node — during the 3 min it ran against the fresh chain it rewound its `blocks.jsonl` to the new
   block 0, so the grind chain's block cache (32,519 lines) is gone from `explorer.grind-20261112/`;
   `blocks.jsonl.old` (3,498 lines, 2026-11-02) and the 123 `focus/<epoch>.json` records survive, and
   the grind state dir can be re-walked by the old binary if ever wanted. The explorer page shows
   the ticket fields only after P15e.
7. 16:30 the watcher (§ watcher) up.

## watcher

runit `alpha-watch` on edge-b, user cyber: `/srv/edge/sv/alpha-watch/{run,alpha-watch.py,log/run}`
(python stdlib), every 600 s one JSON line to
`/srv/edge/data/alpha-watch/alpha-tickets-2026-11-12.jsonl`: `time`, `height`, `tip`, `chainwork`,
`bits`, `blocks_since`/`secs_since`/`block_secs`, `expected_attempts` (2^256 / target at the tip's bits),
`tickets_per_s_chain` (expected attempts × blocks / seconds — an estimate, never consensus),
`tickets_per_s_local` (the last rate line of the edge miner's log), `peers` (agent-a's `best_height`
and `best_tip` as edge-b sees them — agent-a's HTTP is loopback behind NAT, so the peer view is the
pair check; `--peer-url` records a second node directly when one is reachable), `economy`
(`epoch`, `last_settled`, `pool`, `active_stake`), `epoch_records` (the `digest` of the newest three
settled `/epoch/E`), `lottery_1_sha256` once `/epoch/1/lottery` answers (after height 600), `errors`.
Logs `/srv/edge/log/alpha-watch/current`. The first line: 16:30:04 height 0, bits 0x20020000, no peers.
agent-a's epoch digests for the equality check: `curl -s 127.0.0.1:7783/epoch/E | python3 -c
'import json,sys; print(json.load(sys.stdin)["digest"])'` on agent-a against the file's digest.

## state at the end of the run (2026-11-12 16:45 UTC)

- block 1 `e94692b9…e101` at 16:42:55, mined on edge-b after 7 attempts (the pair's first ticket
  block, 13 min after the miners started: ≈128 expected attempts at 2^249 against ≈0.3 tickets/s
  on the two hosts); block 2 `249c9aec…fa06` at 16:44:04 (16 attempts, 65 s); block 3
  `4f299ef1…1400` at 16:44:4x. agent-a's node logged `header e94692b9 at 1 from 54471c98` and
  `header 249c9aec at 2` within 0.1 s of each, its `/tip` equal to edge-b's at 2 and 3, its miner
  `tip moved to height 2 — refreshing`: **the pair advances together on the same block 0**.
- bits moved from 0x20020000 to 0x20100000 (`POW_LIMIT`, 2^252) at height 3: ASERT anchors at the
  genesis time 1793523600 (2026-11-01 09:00, kept from the stand), eleven days behind the clock, so the
  chain mines at the floor (≈16 expected attempts, about one block a minute at the pair's rate)
  until the schedule catches up — it will not within 24 h. First settlement at 600 ≈ 10 h in; the
  watcher's `block_secs` shows the actual cadence. A restart with `--time` nearer the clock is a
  PR to `conformance/chain-genesis.toml` `[pussy-rc]`, not an edit here.
- edge-b: `cybernode-check` all OK but the systemd row; `/economy` epoch 0, `last_settled` 0,
  pool 0, `active_stake` 0 (no stakes on the alpha yet). agent-a: 1 outbound peer, score 0.
- the watcher's second line (16:40) reads `tickets_per_s_local: null`: the miner's log dir
  `/srv/edge/log/soft3-mine-pussy-rc` was 0700 root; made 0755 (`current` is 0644) at 16:44, a
  `--once` run as cyber then read 0.2 tickets/s. The miner log file carries the grind miner's lines
  before 16:30 (`grep accepted` counts both chains).

## the 24 h (to append from the watcher's file)

block times, tickets/s per host, verification time, epoch records agreement at 360 and 600, the
first payout at 600 (`/epoch/1/lottery`), any halt or reorg.
