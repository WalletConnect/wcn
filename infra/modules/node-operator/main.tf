terraform {
  required_providers {
    sops = {
      source  = "carlpett/sops"
    }
  }
}

variable "config" {
  type = object({
    name    = string
    secrets_file_path = string

    db = object({
      ec2_instance_type = string

      ebs_volume_size = number

      ecs_task_container_image = string
      ecs_task_cpu             = number
      ecs_task_memory          = number
    })
  })
}

data "aws_region" "current" {}

ephemeral "sops_file" "secrets" {
  source_file = var.config.secrets_file_path
}

locals {
  region = data.aws_region.current.name

  # We store encrypted secrets as a `local` to be able to derive secret versions.
  encrypted_secrets = yamldecode(file(var.config.secrets_file_path))

  # The decrypted secrets are not being stored in the TF state as they are `ephemeral`.
  secrets = yamldecode(ephemeral.sops_file.secrets.raw)

  db = {
    primary_rpc_server_port   = 3000
    secondary_rpc_server_port = 3001
    metrics_server_port       = 3002
  }
}

resource "aws_ssm_parameter" "ed25519_secret_key" {
  name             = "${var.config.name}-ed25519-secret-key"
  type             = "SecureString"
  value_wo         = local.secrets.ed25519_secret_key
  value_wo_version = parseint(substr(sha1(local.encrypted_secrets.ed25519_secret_key), 0, 8), 16)
}

module "vpc" {
  source  = "terraform-aws-modules/vpc/aws"
  version = "~> 6.5"

  name = var.config.name
  cidr = "10.0.0.0/16"
  region = local.region

  azs             = ["${local.region}a", "${local.region}b"]
  private_subnets = ["10.0.1.0/24"]
  public_subnets  = ["10.0.101.0/24", "10.0.102.0/24"]
}

module "db" {
  source = "../db"
  config = merge(var.config.db, local.db, {
    operator_name = var.config.name

    vpc    = module.vpc
    subnet = module.vpc.private_subnet_objects[0]

    secret_key_arn = aws_ssm_parameter.ed25519_secret_key.arn
    secrets_version = sha1(jsonencode([
      local.encrypted_secrets.ed25519_secret_key
    ]))
  })
}
