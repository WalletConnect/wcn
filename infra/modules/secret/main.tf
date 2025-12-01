variable "name" {
  type = string
}

variable "value" {
  type = string
}

variable "value_encrypted" {
  type = string
}

local {
  version = parseint(substr(sha1(var.value_encrypted), 0, 8), 16)
}

resource "aws_ssm_parameter" "this" {
  name             = var.name
  type             = "SecureString"
  value_wo         = var.value
  value_wo_version = local.version
}

output "ssm_parameter_arn" {
  value = aws_ssm_parameter.this.arn
}

output "version" {
  value = local.version
}
