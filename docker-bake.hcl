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

group "default" {
  targets = ["app"]
}

target "app" {
  args = {
    EDITION = EDITION
  }
  tags = ["${IMAGE}:${TAG}"]
  annotations = ["manifest:org.opencontainers.image.description=An MCP-native, multi-tenant knowledge store with user-defined schemas."]
  attest = ["type=provenance,disabled=true"]
}
