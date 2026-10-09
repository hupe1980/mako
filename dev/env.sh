# Sourced with a service name by `just dev`, `just run` and dev/smoke.sh:
# exports what the service's dev config cannot hold because it depends on where
# the checkout is.
#
#   . dev/env.sh marktd
dev_env() {
    local svc="$1" root
    root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
    local prefix
    prefix="$(echo "$svc" | tr 'a-z-' 'A-Z_')"
    export "${prefix}_CONFIG=$root/dev/config/$svc.toml"
    case "$svc" in
        # The Iceberg catalog lives in the edmd database and outlives a restart,
        # so the warehouse has to as well — outside target/, which `cargo clean`
        # empties while the database keeps pointing into it. `just infra-reset`
        # deletes both. A file:// URI must be absolute.
        edmd)
            mkdir -p "$root/dev/.data/edmd-warehouse"
            export "EDMD_ARCHIVE__STORAGE_URI=file://$root/dev/.data/edmd-warehouse"
            ;;
    esac
}
dev_env "$1"
