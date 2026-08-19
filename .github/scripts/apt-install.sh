#!/usr/bin/env bash
# Install apt packages on a hosted runner without hanging on a dead mirror.
#
# The runners resolve archive.ubuntu.com through azure.archive.ubuntu.com,
# which intermittently stops answering. apt spends ~40s retrying it, falls
# back to the canonical archive, and has been observed to stall there until
# the job is killed. Repoint at the canonical host, bound every fetch, and
# retry the operation rather than waiting on a single attempt.
set -euo pipefail

for f in /etc/apt/apt-mirrors.txt /etc/apt/sources.list; do
    if [ -f "$f" ]; then
        sudo sed -i 's|azure\.archive\.ubuntu\.com|archive.ubuntu.com|g' "$f"
    fi
done

sudo tee /etc/apt/apt.conf.d/99-ci-timeouts >/dev/null <<'CONF'
Acquire::Retries "3";
Acquire::http::Timeout "30";
Acquire::https::Timeout "30";
CONF

attempt() {
    local n=1
    until "$@"; do
        if [ "$n" -ge 3 ]; then
            echo "::error::apt failed after $n attempts: $*"
            return 1
        fi
        echo "::warning::apt attempt $n failed, retrying: $*"
        n=$((n + 1))
        sleep $((n * 5))
    done
}

attempt sudo apt-get update
attempt sudo apt-get install -y "$@"
