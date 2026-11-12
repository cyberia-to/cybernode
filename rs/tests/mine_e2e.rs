//! `cybernode mine` against a mock chain node: `GET /template`, `GET /game`,
//! `GET /tip`, `POST /submit` (chain-wire §7 shapes, amended for the ticket).
//! The mock serves a template at `EASY` (2^255, one attempt in two) over a
//! small game of three players, verifies every submitted header natively
//! — the id under the target and the sample recomputed on the same game
//! (rewards.md §7) — advances its tip on acceptance, answers `409` to a
//! header of the old tip, and drops the first template request so the
//! client's retry is exercised.

use cybernode::chain::header::{Header, encode_bits};
use cybernode::chain::mine::api::{hex, unhex, Accepted, GameSummary, Submit, Template, Tip};
use cybernode::chain::mine::{mine_template, Outcome, Settings};
use cybernode::chain::mine::api::Client;
use cybernode::chain::pow::id_meets_target;
use cybernode::chain::ticket::{EpochGame, GameWire, Player};
use cybernode::chain::u256::U256;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 2^255, canonical: one attempt in two passes.
const EASY: u32 = 0x2100_8000;

fn game() -> EpochGame {
    let pid = |i: u8| [i; 32];
    let base = vec![(pid(1), pid(2), 1_000_000u128), (pid(2), pid(3), 2_000_000), (pid(3), pid(1), 500_000)];
    let players = vec![
        Player { card: [10; 32], side: 1, delta: 300_000, p: pid(1), q: pid(2), owner: [7; 32] },
        Player { card: [11; 32], side: -1, delta: 200_000, p: pid(2), q: pid(3), owner: [7; 32] },
        Player { card: [12; 32], side: 1, delta: 400_000, p: pid(2), q: pid(4), owner: [8; 32] },
    ];
    EpochGame::build(0, [0xbe; 32], base, players, 3, 2).unwrap()
}

struct Node {
    height: u64,
    prev: [u8; 32],
    chain: [u8; 32],
    game: EpochGame,
    accepted: Vec<Header>,
}

impl Node {
    fn template(&self) -> Template {
        let h = Header {
            version: 1,
            height: self.height + 1,
            time: 1_793_885_850,
            bits: EASY,
            nonce: [0; 16],
            prev: self.prev,
            tx_root: [0x22; 32],
            state_root: [0x33; 32],
            chain: self.chain,
            sample: [0; 32],
        };
        Template {
            chain: "pussy".into(),
            height: self.height + 1,
            prev: hex(&self.prev),
            header: hex(&h.encode()),
            bits: format!("{EASY:08x}"),
            target: hex(&(U256::ONE << 255).to_le_bytes()),
            body: "01000000".into(),
            expires: 0,
            game: GameSummary { epoch: 0, beacon: hex(&self.game.beacon), cluster: hex(&self.game.cluster), players: 3, axons: 3, w_ticket: 3 },
        }
    }
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
}

fn serve(listener: TcpListener, node: Arc<Mutex<Node>>, drops: Arc<AtomicU32>) {
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { break };
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        loop {
            let n = stream.read(&mut tmp).unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
            if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..end]).to_string();
                let len = head.lines().find_map(|l| l.strip_prefix("Content-Length: ")).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
                if buf.len() >= end + 4 + len {
                    break;
                }
            }
        }
        let text = String::from_utf8_lossy(&buf).to_string();
        let line = text.lines().next().unwrap_or("").to_string();
        let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        let mut n = node.lock().unwrap();
        if line.starts_with("GET /template") {
            if drops.load(Relaxed) > 0 {
                drops.fetch_sub(1, Relaxed);
                drop(stream); // connection reset: the client retries
                continue;
            }
            assert!(line.contains("?to=") && line.contains(&"ab".repeat(32)), "{line}");
            respond(&mut stream, "200 OK", &serde_json::to_string(&n.template()).unwrap());
        } else if line.starts_with("GET /game/") {
            assert!(line.contains(&hex(&n.game.beacon)), "{line}");
            respond(&mut stream, "200 OK", &GameWire::of(&n.game).to_json());
        } else if line.starts_with("GET /tip") {
            let t = Tip { height: n.height, id: hex(&n.prev), chainwork: "0".into() };
            respond(&mut stream, "200 OK", &serde_json::to_string(&t).unwrap());
        } else if line.starts_with("POST /submit") {
            let s: Submit = serde_json::from_str(&body).expect("submit json");
            let h = Header::decode_raw(&unhex(&s.header).unwrap()).unwrap();
            if h.prev != n.prev {
                respond(&mut stream, "409 Conflict", "{\"error\":\"stale template\"}");
            } else if h.bits != EASY || !id_meets_target(&h.id(), U256::ONE << 255) {
                respond(&mut stream, "422 Unprocessable Entity", "{\"error\":\"pow\"}");
            } else if n.game.verify(&h, 2).is_err() {
                respond(&mut stream, "422 Unprocessable Entity", "{\"error\":\"sample\"}");
            } else {
                let id = h.id();
                n.height += 1;
                n.prev = id;
                n.accepted.push(h);
                let a = Accepted { id: hex(&id), height: n.height };
                respond(&mut stream, "200 OK", &serde_json::to_string(&a).unwrap());
            }
        } else {
            respond(&mut stream, "404 Not Found", "{}");
        }
    }
}

#[test]
fn mines_two_blocks_against_a_mock_node_with_retry_and_stale_submit() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let node = Arc::new(Mutex::new(Node { height: 0, prev: [0x11; 32], chain: [0x44; 32], game: game(), accepted: Vec::new() }));
    let drops = Arc::new(AtomicU32::new(1));
    {
        let (node, drops) = (node.clone(), drops.clone());
        std::thread::spawn(move || serve(listener, node, drops));
    }
    let settings = Settings {
        chain: "pussy".into(),
        node: url.clone(),
        address: "ab".repeat(32),
        workers: 2,
        tip_poll: Duration::from_millis(200),
        refresh: Duration::from_secs(60),
        report: Duration::from_millis(500),
    };
    let client = Client::new(&url, &settings.address);
    let log = |m: &str| eprintln!("{m}");
    let stop = AtomicBool::new(false);

    // first template: the first request is dropped, the retry gets it
    let t1 = Client::retry(|| client.template(), 0, Duration::from_secs(1), log).unwrap();
    assert_eq!(drops.load(Relaxed), 0);
    assert_eq!(t1.height, 1);
    assert_eq!(t1.bits_u32().unwrap(), EASY);
    assert_eq!(encode_bits(t1.target_u256().unwrap()), EASY);
    // the game the template names, rebuilt by the miner: the same cluster
    let wire = client.game(&t1.game.beacon).unwrap();
    let g = wire.build(2).unwrap();
    assert_eq!(hex(&g.cluster), t1.game.cluster);
    let o = mine_template(&client, &g, &t1, 0xf00d, &settings, &stop, &log).unwrap();
    let Outcome::Accepted { height, nonce, attempts, .. } = o else { panic!("{o:?}") };
    assert_eq!(height, 1);
    assert_eq!(nonce >> 64, 0xf00d);
    assert!(attempts >= 1);
    {
        let n = node.lock().unwrap();
        assert_eq!(n.accepted.len(), 1);
        assert_eq!(n.accepted[0].nonce, nonce.to_le_bytes());
        assert!(n.game.verify(&n.accepted[0], 2).is_ok(), "the submitted sample is the ticket of its template");
    }

    // mining the old template again: the tip moved, so the miner refreshes
    // (tip poll) — or, if it finds first, the node says 409 and it refreshes
    let o = mine_template(&client, &g, &t1, 0xf00e, &settings, &stop, &log).unwrap();
    assert_eq!(o, Outcome::Refresh);

    // the fresh template mines block 2 on top of block 1
    let t2 = client.template().unwrap();
    assert_eq!(t2.height, 2);
    assert_ne!(t2.prev, t1.prev);
    let o = mine_template(&client, &g, &t2, 0xf00f, &settings, &stop, &log).unwrap();
    assert!(matches!(o, Outcome::Accepted { height: 2, .. }), "{o:?}");
    assert_eq!(node.lock().unwrap().accepted.len(), 2);
}
