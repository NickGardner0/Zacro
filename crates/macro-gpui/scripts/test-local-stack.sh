#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
ZED_ROOT=$(cd "$SCRIPT_DIR/../../.." && pwd)
MACRO_SOURCE_DIR=${MACRO_SOURCE_DIR:-"$(cd "$ZED_ROOT/.." && pwd)/macro"}
INSTANCE=${MACRO_GPUI_E2E_INSTANCE:-macro-gpui}
REUSE=false

if [[ ${1:-} == "--reuse" ]]; then
  REUSE=true
  shift
fi

if [[ ! -x "$MACRO_SOURCE_DIR/tooling/scripts/run-local-e2e.sh" ]]; then
  echo "Macro source checkout not found at $MACRO_SOURCE_DIR" >&2
  exit 2
fi

if [[ "$REUSE" == false ]]; then
  "$MACRO_SOURCE_DIR/tooling/scripts/run-local-e2e.sh" \
    --instance "$INSTANCE" \
    --suite rust
fi

ARTIFACT_DIR="$MACRO_SOURCE_DIR/infra/local/generated/$INSTANCE"
ENV_FILE="$ARTIFACT_DIR/local.generated.env"
PORTMAP="$ARTIFACT_DIR/portmap.json"
if [[ ! -f "$ENV_FILE" || ! -f "$PORTMAP" ]]; then
  echo "Local E2E artifacts are missing for instance $INSTANCE" >&2
  exit 2
fi

PROJECT="macro-$INSTANCE"
PROXY_CONTAINER=$(docker compose -p "$PROJECT" ps -q proxy)
if [[ -z "$PROXY_CONTAINER" ]]; then
  echo "The Macro proxy container for $PROJECT is not running" >&2
  exit 2
fi
PROXY_PORT=$(docker port "$PROXY_CONTAINER" | awk -F: 'NR == 1 {print $NF}')
if [[ -z "$PROXY_PORT" ]]; then
  echo "Could not resolve the Macro proxy port" >&2
  exit 2
fi

LOCAL_E2E_BASE_URL=$(python3 -c \
  'import json,sys; print(json.load(open(sys.argv[1]))["webAppUrl"] + "/app")' \
  "$PORTMAP")
LOCAL_E2E_BACKEND_ORIGIN="http://localhost:$PROXY_PORT"
LOCAL_E2E_TOKEN=$(
  cd "$MACRO_SOURCE_DIR"
  LOCAL_E2E_ENV_FILE="$ENV_FILE" cargo run --quiet \
    -p local_e2e_test_support \
    --bin generate_local_e2e_token
)

cd "$ZED_ROOT"
export LOCAL_E2E_BACKEND_ORIGIN
export LOCAL_E2E_BASE_URL
export LOCAL_E2E_TOKEN
export LOCAL_E2E_ENV_FILE="$ENV_FILE"
export MACRO_SOURCE_DIR
export CARGO_INCREMENTAL=0

cargo test -p macro_api --test local_stack \
  -- --ignored --nocapture "$@"
