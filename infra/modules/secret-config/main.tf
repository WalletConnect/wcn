variable "name" {
  type = string
}

variable "template" {
  type = string
}

variable "ephemeral_args" {
  type = map(string)
  ephemeral = true
  default = null
}

variable "args" {
  type = map(string)
}

locals {
  ephemeral_value = var.ephemeral_args == null ? null : templatestring(var.template, var.ephemeral_args)
  value = templatestring(var.template, var.args)

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
