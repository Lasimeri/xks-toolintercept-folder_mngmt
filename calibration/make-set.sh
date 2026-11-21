#!/usr/bin/env bash
# make-set.sh REPOS_DIR: draw a file.root calibration set from real
# repositories checked out under REPOS_DIR (Intel-Phi-Jev, Mechanical-Jev,
# XKS_Jev-like_Model_Base, XKS-Memory, xks-id, x1tt3r; all public under
# github.com/Lasimeri). For each, its manifest in manifests/ declares (by
# hand, before any run) which role each top-level folder plays; root files
# are `config`. Up to 4 tracked files per top-level group are taken, evenly
# spread, skipping binary files (a NUL in the first 4 KB) and the .md notes
# kept beside code in source folders. Generated files from the base's out/
# (untracked run outputs, present after its parity and bench runs) are added
# with gold `out`. Each line names its repository by folder name only; give
# `xtf calibrate` the same REPOS_DIR with --repos.
#
# Writes file-root.new.jsonl next to this script. The committed
# file-root.jsonl is the set the published results were measured on; a new
# draw differs where the repositories (and the base's out/) have changed.
set -euo pipefail
if [ $# -ne 1 ] || [ ! -d "$1" ]; then
    echo "usage: $0 REPOS_DIR (the folder holding the repositories)" >&2
    exit 2
fi
repos=$(cd "$1" && pwd)
cd "$(dirname "$0")"
out=file-root.new.jsonl
: >"$out"

text() { # true when the first 4 KB hold no NUL byte
    [ "$(head -c 4096 "$1" | tr -d '\000' | wc -c)" = "$(head -c 4096 "$1" | wc -c)" ]
}

emit() { # manifest-id repository path gold
    jq -cn --arg m "manifests/$1.json" --arg r "$2" --arg p "$3" --arg g "$4" \
        '{manifest: $m, root: $r, path: $p, gold: $g}' >>"$out"
}

for pair in intel-phi-jev:Intel-Phi-Jev mechanical-jev:Mechanical-Jev \
    xks-jev-base:XKS_Jev-like_Model_Base xks-memory:XKS-Memory xks-id:xks-id x1tt3r:x1tt3r; do
    id=${pair%%:*}
    name=${pair#*:}
    root=$repos/$name
    if [ ! -d "$root/.git" ]; then
        echo "skipped: $name is not checked out under $repos" >&2
        continue
    fi
    # folder -> role, from the manifest
    declare -A role=()
    while IFS=$'\t' read -r dir r; do role[$dir]=$r; done < <(
        jq -r '.layout | to_entries[] | .key as $r | .value[] | "\(.)\t\($r)"' "manifests/$id.json")
    declare -A seen=()
    groups=()
    while IFS= read -r f; do
        top=${f%%/*}
        [ "$top" = "$f" ] && top="(root)"
        if [ "$top" = "(root)" ]; then gold=config; else gold=${role[$top]:-}; fi
        [ -n "$gold" ] || continue
        [ "$gold" = source ] && [[ $f == *.md ]] && continue
        [ -f "$root/$f" ] && text "$root/$f" || continue
        [ -n "${seen[$top]:-}" ] || groups+=("$top")
        seen[$top]+="$f"$'\n'
    done < <(git -C "$root" ls-files)
    for top in "${groups[@]}"; do
        mapfile -t files < <(printf '%s' "${seen[$top]}")
        n=${#files[@]}
        k=$((n < 4 ? n : 4))
        for ((i = 0; i < k; i++)); do
            f=${files[$((i * n / k))]}
            if [ "$top" = "(root)" ]; then g=config; else g=${role[$top]}; fi
            emit "$id" "$name" "$f" "$g"
        done
    done
    unset role seen
done

# Generated output of the base (untracked): logs, run results, snapshots.
base=$repos/XKS_Jev-like_Model_Base
if [ -d "$base/out" ]; then
    mapfile -t gen < <(cd "$base" && find out -type f \( -name '*.log' -o -name '*.txt' -o -name '*.jsonl' \) | sort)
    n=${#gen[@]}
    k=$((n < 6 ? n : 6))
    for ((i = 0; i < k; i++)); do
        f=${gen[$((i * n / k))]}
        text "$base/$f" && emit xks-jev-base XKS_Jev-like_Model_Base "$f" out
    done
fi
echo "$(wc -l <"$out") items -> calibration/$out"
