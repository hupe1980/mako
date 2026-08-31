# docker-bake.hcl — builds all 17 mako service images from one build graph.
#
# This file is the **CI** build path: `_base` fixes `push-by-digest`, so every
# target pushes to `$REGISTRY` and none of them loads an image into the local
# daemon. `docker buildx bake <target>` therefore fails on the default docker
# driver with "push-by-digest is currently not implemented" — it is not the
# command to reach for locally.
#
# For a local image the demos can run, use `just build-demo`, which tags
# `makod:dev` / `marktd:dev` / `processd:dev` the way the compose files expect.
#
# CI (per-platform, native runner):
#   docker buildx bake \
#     --set "*.platform=linux/amd64" \
#     --set "*.cache-from=type=registry,ref=ghcr.io/hupe1980/mako-builder:linux-amd64" \
#     --metadata-file /tmp/bake-meta.json \
#     --push
#
# The `_base` target is inherited by all 17 service targets.
# BuildKit executes the shared `builder` stage once and fans out to 17 runtime stages.

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
    "netzbilanzd", "sperrd", "einsd",
    "productd", "billingd", "outputd", "accountingd", "vertragd",
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
  tags     = ["${REGISTRY}/mako-makod"]
}

target "marktd" {
  inherits = ["_base"]
  target   = "marktd-runtime"
  tags     = ["${REGISTRY}/mako-marktd"]
}

target "processd" {
  inherits = ["_base"]
  target   = "processd-runtime"
  tags     = ["${REGISTRY}/mako-processd"]
}

target "invoicd" {
  inherits = ["_base"]
  target   = "invoicd-runtime"
  tags     = ["${REGISTRY}/mako-invoicd"]
}

target "edmd" {
  inherits = ["_base"]
  target   = "edmd-runtime"
  tags     = ["${REGISTRY}/mako-edmd"]
}

target "obsd" {
  inherits = ["_base"]
  target   = "obsd-runtime"
  tags     = ["${REGISTRY}/mako-obsd"]
}

target "netzbilanzd" {
  inherits = ["_base"]
  target   = "netzbilanzd-runtime"
  tags     = ["${REGISTRY}/mako-netzbilanzd"]
}

target "sperrd" {
  inherits = ["_base"]
  target   = "sperrd-runtime"
  tags     = ["${REGISTRY}/mako-sperrd"]
}

target "einsd" {
  inherits = ["_base"]
  target   = "einsd-runtime"
  tags     = ["${REGISTRY}/mako-einsd"]
}

target "productd" {
  inherits = ["_base"]
  target   = "productd-runtime"
  tags     = ["${REGISTRY}/mako-productd"]
}

target "billingd" {
  inherits = ["_base"]
  target   = "billingd-runtime"
  tags     = ["${REGISTRY}/mako-billingd"]
}

target "outputd" {
  inherits = ["_base"]
  target   = "outputd-runtime"
  tags     = ["${REGISTRY}/mako-outputd"]
}

target "accountingd" {
  inherits = ["_base"]
  target   = "accountingd-runtime"
  tags     = ["${REGISTRY}/mako-accountingd"]
}

target "vertragd" {
  inherits = ["_base"]
  target   = "vertragd-runtime"
  tags     = ["${REGISTRY}/mako-vertragd"]
}

target "portald" {
  inherits = ["_base"]
  target   = "portald-runtime"
  tags     = ["${REGISTRY}/mako-portald"]
}

target "agentd" {
  inherits = ["_base"]
  target   = "agentd-runtime"
  tags     = ["${REGISTRY}/mako-agentd"]
}

target "mabis-syncd" {
  inherits = ["_base"]
  target   = "mabis-syncd-runtime"
  tags     = ["${REGISTRY}/mako-mabis-syncd"]
}
