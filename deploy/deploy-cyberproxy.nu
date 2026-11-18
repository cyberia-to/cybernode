#!/usr/bin/env nu
# Deploy the cybernode binary (spacepussy-test chaosnet, cybergraph+bbg) to cyberproxy.
# Legacy stand of the 0.10.0 chaosnet on 7780; the root chain is deploy/chain/.

const HOST = "cyberproxy"

# rev: the revision to install (the phase-1 pin of cybernode); the crates.io name is not published, so never an unpinned install
def main [rev: string] {
    print $"deploying cybernode ($rev) → ($HOST)"
    ^ssh $HOST $"export PATH=$HOME/.cargo/bin:$PATH; cargo install --git https://github.com/cyberia-to/cybernode --rev ($rev) cybernode --force --locked"
    ^scp "deploy/spacepussy-test.service" $"($HOST):/tmp/spacepussy-test.service"
    ^ssh $HOST 'sudo cp /tmp/spacepussy-test.service /etc/systemd/system/spacepussy-test.service'
    ^ssh $HOST 'sudo systemctl daemon-reload && sudo systemctl enable --now spacepussy-test && sudo systemctl restart spacepussy-test'
    ^ssh $HOST 'systemctl --no-pager status spacepussy-test | head -18'
    ^ssh $HOST 'curl -sS http://127.0.0.1:7780/status | head -c 500; echo'
    print "public: https://cyb.ai/spacepussy-test/status"
}
