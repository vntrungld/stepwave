#!/usr/bin/env bash
# Build dist/stepwave-setup-<version>.exe: stepwave.exe (from the latest green CI run of a
# branch, or --exe), profiles/*.json and the models those profiles reference (local only;
# models are git-ignored and derived from game audio — never publish the result).
#
# Usage: scripts/build-windows-installer.sh [--branch <b>] [--exe <path>] [--no-models] [--out <dir>]
# Needs: makensis, or Docker (uses installer/Dockerfile); gh (unless --exe is given).
set -euo pipefail

root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
branch=$(git -C "$root" branch --show-current)
exe=""
with_models=1
out_dir="dist"
while [ $# -gt 0 ]; do
  case "$1" in
    --branch) branch=$2; shift 2 ;;
    --exe) exe=$2; shift 2 ;;
    --no-models) with_models=0; shift ;;
    --out) out_dir=$2; shift 2 ;;
    -h|--help) sed -n '2,8p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

# Resolve user paths against the caller's directory before changing into the repo.
if [ -n "$exe" ]; then exe=$(realpath "$exe"); fi
mkdir -p "$out_dir"
out_dir=$(realpath "$out_dir")
cd "$root"

build="build/installer"
rm -rf "$build"
mkdir -p "$build/stage/profiles" "$build/stage/models"

# 1. stepwave.exe
sha="local"
if [ -z "$exe" ]; then
  run=$(gh run list --branch "$branch" --workflow CI --status success --limit 1 \
        --json databaseId,headSha -q '.[0] // empty | "\(.databaseId) \(.headSha)"')
  if [ -z "$run" ]; then
    # The windows CI job runs only on main and on pull requests.
    echo "no successful CI run on branch '$branch'" >&2
    exit 1
  fi
  run_id=${run%% *}
  sha=${run##* }
  sha=${sha:0:7}
  echo "downloading stepwave.exe from CI run $run_id ($sha)"
  gh run download "$run_id" -n stepwave-windows -D "$build/ci"
  exe="$build/ci/stepwave.exe"
fi
cp "$exe" "$build/stage/stepwave.exe"

# 2. profiles and the models they reference
cp profiles/*.json "$build/stage/profiles/"
if [ "$with_models" = 1 ]; then
  for profile in profiles/*.json; do
    model=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("model",""))' "$profile")
    if [ -z "$model" ]; then continue; fi
    case "$model" in
      models/*.swm) ;;
      *) echo "error: $profile: model must be models/<name>.swm, got '$model'" >&2; exit 1 ;;
    esac
    if [ -f "$model" ]; then
      cp "$model" "$build/stage/$model"
      echo "including $model"
    else
      echo "warning: $profile references $model, which is missing; that profile will use its static EQ" >&2
    fi
  done
fi

# 3. makensis (native or Docker). makensis changes into the script's directory, so pass
# absolute paths; inside Docker the repo is mounted at /w. It writes into $build, and the
# result is moved to $out_dir afterwards (which Docker may not see).
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
outfile="stepwave-setup-$version-$sha.exe"
nsis_args() {
  local base=$1
  args=(-V2 "-DSTAGE=$base/$build/stage" "-DVERSION=$version-$sha"
    "-DOUTFILE=$base/$build/$outfile" "$base/installer/stepwave.nsi")
}
if command -v makensis >/dev/null; then
  nsis_args "$root"
  makensis "${args[@]}"
else
  docker build -q -t stepwave-nsis installer >/dev/null
  nsis_args /w
  docker run --rm --user "$(id -u):$(id -g)" -v "$root:/w" -w /w stepwave-nsis makensis "${args[@]}"
fi
mv "$build/$outfile" "$out_dir/$outfile"
echo "built $out_dir/$outfile"
