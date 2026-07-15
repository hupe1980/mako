# docker-bake.hcl — builds all six mako service images from one build graph.
#
# Usage (local):
#   VERSION=dev docker buildx bake
#
# CI (per-platform, native runner):
#   docker buildx bake \
#     --set "*.platform=linux/amd64" \
#     --set "*.cache-from=type=registry,ref=ghcr.io/hupe1980/mako-builder:linux-amd64" \
#     --metadata-file /tmp/bake-meta.json \
#     --push
#
# The `_base` target is inherited by all six service targets.
# BuildKit executes the shared `builder` stage once and fans out to six runtime stages.

variable "REGISTRY" {
  default = "ghcr.io/hupe1980"
}

variable "VERSION" {
  default = "dev"
}

variable "OCI_REVISION" {
  default = "unknown"
}

variable "OCI_CREATED" {
  default = "unknown"
}

# Build all 17 images
group "default" {
  targets = [
    "makod", "marktd", "processd", "invoicd", "edmd", "obsd",
    "netzbilanzd", "sperrd", "nis-syncd", "einsd",
    "tarifbd", "billingd", "accountingd", "vertragd",
    "portald", "agentd", "mabis-syncd",
  ]
}

# ── Shared base ───────────────────────────────────────────────────────────────
# `platform` is NOT set here — callers supply it via --set "*.platform=..."
# so that native runners compile for their own arch without QEMU.
target "_base" {
  context    = "."
  dockerfile = "Dockerfile"
  # Push each image by OCI digest (no tag yet).
  # The docker-manifest CI job attaches semver tags from both platform digests.
  output = ["type=image,push-by-digest=true,name-canonical=true,push=true"]
  args = {
    OCI_REVISION = "${OCI_REVISION}"
    OCI_CREATED  = "${OCI_CREATED}"
    OCI_VERSION  = "${VERSION}"
  }
}

# ── Service targets ───────────────────────────────────────────────────────────

target "makod" {
  inherits = ["_base"]
  target   = "runtime"
  tags     = ["${REGISTRY}/makod"]
}

target "marktd" {
  inherits = ["_base"]
  target   = "marktd-runtime"
  tags     = ["${REGISTRY}/marktd"]
}

target "processd" {
  inherits = ["_base"]
  target   = "processd-runtime"
  tags     = ["${REGISTRY}/processd"]
}

target "invoicd" {
  inherits = ["_base"]
  target   = "invoicd-runtime"
  tags     = ["${REGISTRY}/invoicd"]
}

target "edmd" {
  inherits = ["_base"]
  target   = "edmd-runtime"
  tags     = ["${REGISTRY}/edmd"]
}

target "obsd" {
  inherits = ["_base"]
  target   = "obsd-runtime"
  tags     = ["${REGISTRY}/obsd"]
}

target "netzbilanzd" {
  inherits = ["_base"]
  target   = "netzbilanzd-runtime"
  tags     = ["${REGISTRY}/netzbilanzd"]
}

target "sperrd" {
  inherits = ["_base"]
  target   = "sperrd-runtime"
  tags     = ["${REGISTRY}/sperrd"]
}

target "nis-syncd" {
  inherits = ["_base"]
  target   = "nis-syncd-runtime"
  tags     = ["${REGISTRY}/nis-syncd"]
}

target "einsd" {
  inherits = ["_base"]
  target   = "einsd-runtime"
  tags     = ["${REGISTRY}/einsd"]
}

target "tarifbd" {
  inherits = ["_base"]
  target   = "tarifbd-runtime"
  tags     = ["${REGISTRY}/tarifbd"]
}

target "billingd" {
  inherits = ["_base"]
  target   = "billingd-runtime"
  tags     = ["${REGISTRY}/billingd"]
}

target "accountingd" {
  inherits = ["_base"]
  target   = "accountingd-runtime"
  tags     = ["${REGISTRY}/accountingd"]
}

target "vertragd" {
  inherits = ["_base"]
  target   = "vertragd-runtime"
  tags     = ["${REGISTRY}/vertragd"]
}

target "portald" {
  inherits = ["_base"]
  target   = "portald-runtime"
  tags     = ["${REGISTRY}/portald"]
}

target "agentd" {
  inherits = ["_base"]
  target   = "agentd-runtime"
  tags     = ["${REGISTRY}/agentd"]
}

target "mabis-syncd" {
  inherits = ["_base"]
  target   = "mabis-syncd-runtime"
  tags     = ["${REGISTRY}/mabis-syncd"]
}
