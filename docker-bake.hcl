# docker-bake.hcl — one image per service, every image from one build graph.
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
# `services` below is the one list of released images: `release.yml` reads it
# back with `docker buildx bake --print` to export each digest and tag each
# image, and `ci.yml` lints every stage it names with `docker buildx bake
# --check`.
#
# CI (per-platform, native runner):
#   docker buildx bake \
#     --set "*.platform=linux/amd64" \
#     --set "*.cache-from=type=registry,ref=ghcr.io/hupe1980/mako-builder:linux-amd64" \
#     --metadata-file /tmp/bake-meta.json \
#     --push

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

variable "services" {
  default = [
    "makod", "marktd", "processd", "invoicd", "edmd", "netzbilanzd", "einsd",
    "productd", "billingd", "outputd", "accountingd", "vertragd", "portald",
    "agentd",
  ]
}

group "default" {
  targets = ["service"]
}

# `platform` is NOT set here — callers supply it via --set "*.platform=..."
# so that native runners compile for their own arch without QEMU.
target "_base" {
  context    = "."
  dockerfile = "Dockerfile"
  # Push each image by OCI digest (no tag yet); the docker-manifest job in
  # release.yml attaches the semver tags from both platform digests.
  output = ["type=image,push-by-digest=true,name-canonical=true,push=true"]
  args = {
    OCI_REVISION = "${OCI_REVISION}"
    OCI_CREATED  = "${OCI_CREATED}"
    OCI_VERSION  = "${VERSION}"
  }
}

# One target per service, named after it. makod's stage is the Dockerfile's
# `runtime`; every other service's is `<service>-runtime`.
target "service" {
  matrix   = { svc = services }
  name     = svc
  inherits = ["_base"]
  target   = svc == "makod" ? "runtime" : "${svc}-runtime"
  tags     = ["${REGISTRY}/mako-${svc}"]
}
