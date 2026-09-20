#!/usr/bin/env bash
# Typed demo session recorded by asciinema. Not sourced.
#
#   asciinema rec --overwrite --headless --return \
#     --output-format asciicast-v2 --window-size 100x22 \
#     --command "bash docs/record-session.sh" docs/demo.cast
#   agg --theme dracula --font-size 18 --cols 100 --rows 22 \
#     docs/demo.cast docs/demo.gif
#   ffmpeg -y -i docs/demo.gif -fps_mode cfr -r 12 -pix_fmt yuv420p \
#     -vf "scale=trunc(iw/2)*2:trunc(ih/2)*2" -movflags +faststart docs/demo.mp4
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="$ROOT:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PS1='$ '
export HISTFILE=/dev/null

type_line() {
  local s=$1 i
  printf '$ '
  for ((i = 0; i < ${#s}; i++)); do
    printf '%s' "${s:i:1}"
    sleep 0.025
  done
  printf '\n'
}

run() {
  type_line "$1"
  sleep 0.2
  eval "$1"
  sleep 0.6
}

sleep 0.4
run "echo 'workmbp loopback is not on this laptop until herdr-ports opens it'"
run "/usr/bin/ssh -o BatchMode=yes workmbp 'curl -sS --max-time 2 http://127.0.0.1:9810'"
# local miss is expected
set +e
run "curl -sS --max-time 2 http://127.0.0.1:9810"
set -e
run "herdr-ports add workmbp 9810 --workspace demo"
run "curl -sS --max-time 3 http://herdr.demo.localhost:9810"
run "herdr-ports list"
sleep 2
