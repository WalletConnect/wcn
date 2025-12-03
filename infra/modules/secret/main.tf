variable "name" {
  type = string
}

variable "template" {
  type = string
  default = null
}

variable "ephemeral_value" {
  type = any
  ephemeral = true
  default = null
}

variable "value" {
  type = any
}

locals {
  ephemeral_value = var.secret.value == null ? null : var.template == null ? var.ephemeral_value : templatestring(var.template, var.ephemeral_value)
  value = var.template == null ? var.value : templatestring(var.template, var.value)

  version = parseint(substr(sha1(local.value), 0, 8), 16)  
}

resource "aws_ssm_parameter" "this" {
  name             = var.name
  type             = "SecureString"
  value_wo         = local.ephemeral_value == null ? local.value : local.ephemeral_value
  value_wo_version = local.version
}

output "ssm_parameter_arn" {
  value = aws_ssm_parameter.this.arn
}

output "version" {
  value = local.version
}
