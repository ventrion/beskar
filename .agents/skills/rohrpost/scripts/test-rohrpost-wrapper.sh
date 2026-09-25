#!/usr/bin/env bash
set -euo pipefail

# Self-test for the bash wrapper template. Both binary names the wrapper may
# find under the install home's bin/ are driven end to end, so running this
# under Git Bash on Windows exercises the same resolution path. The PowerShell
# and cmd wrappers are validated on a real machine by playbooks/windows.md.

skill_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
template="$skill_dir/scripts/rohrpost.template"
temp_root="$(mktemp -d)"
trap 'rm -rf "$temp_root"' EXIT

bash -n "$template"

check_layout() {
    local layout="$1"
    rm -rf "$temp_root/bin"
    mkdir -p "$temp_root/bin" "$temp_root/caller"
    printf '%s\n' '#!/usr/bin/env bash' 'printf "cwd=%s args=%s\\n" "$PWD" "$*"' \
        > "$temp_root/bin/$layout"
    chmod +x "$temp_root/bin/$layout"

    sed "s|__ROHRPOST_HOME__|$temp_root|" "$template" > "$temp_root/rohrpost"
    chmod +x "$temp_root/rohrpost"

    local output
    output="$(cd "$temp_root/caller" && "$temp_root/rohrpost" doctor --json)"
    test "$output" = "cwd=$temp_root/caller args=doctor --json"
}

for layout in rp rp.exe; do
    check_layout "$layout"
done

# A missing binary is reported, never bypassed.
rm -rf "$temp_root/bin"
if (cd "$temp_root/caller" && "$temp_root/rohrpost" doctor --json 2>/dev/null); then
    echo "wrapper ran without an installed binary" >&2
    exit 1
fi

printf 'rohrpost wrapper checks passed\n'
