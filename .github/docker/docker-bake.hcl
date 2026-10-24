variable "IMAGE" {
  default = "yorishiro"
}

variable "TAG" {
  default = "local"
}

variable "EDITION" {
  default = "ee"
  validation {
    condition = contains(["ce", "ee"], EDITION)
    error_message = "EDITION must be ce or ee"
  }
}

variable "ARCH" {
  default = "amd64"
  validation {
    condition = contains(["amd64", "arm64"], ARCH)
    error_message = "ARCH must be amd64 or arm64"
  }
}

group "default" {
  targets = ["app"]
}

target "app" {
  context = "."
  dockerfile = "Dockerfile"
  args = {
    EDITION = EDITION
  }
  tags = ["${IMAGE}:${TAG}"]
  annotations = ["manifest:org.opencontainers.image.description=An MCP-native, multi-tenant knowledge store with user-defined schemas."]
  attest = ["type=provenance,disabled=true"]
}

target "release" {
  name = "release-${edition}"
  matrix = {
    edition = ["ce", "ee"]
  }
  inherits = ["app"]
  args = {
    EDITION = edition
  }
  platforms = ["linux/${ARCH}"]
  tags = ["${IMAGE}:${TAG}-${edition}-${ARCH}"]
  cache-from = ["type=gha,scope=${edition}-${ARCH}"]
  cache-to = ["type=gha,mode=max,scope=${edition}-${ARCH}"]
}
