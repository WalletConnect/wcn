variable "name" {
  type = string
}

variable "ephemeral_value" {
  type = string
  ephemeral = true
  default = null
}

variable "value" {
  type = string
}

locals {
  version = parseint(substr(sha1(var.value), 0, 8), 16)  
}

resource "aws_ssm_parameter" "this" {
  name             = var.name
  type             = "SecureString"
  value_wo         = var.ephemeral_value == null ? var.value : var.ephemeral_value
  value_wo_version = local.version
}

output "ssm_parameter_arn" {
  value = aws_ssm_parameter.this.arn
}

output "version" {
  value = local.version
}
