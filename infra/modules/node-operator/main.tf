variable "config" {
  type = object({
    name    = string
    peer_id = string

    db = object({
      ec2_instance_type = string

      ebs_volume_size = number

      ecs_task_container_image = string
      ecs_task_cpu             = number
      ecs_task_memory          = number
    })
  })
}

variable "secrets" {
  type = object({
    ed25519_secret_key = string
  })
  sensitive = true
  ephemeral = true
}

locals {
  region = data.aws_region.current.name

  db = {
    primary_rpc_server_port   = 3000
    secondary_rpc_server_port = 3001
    metrics_server_port       = 3002
  }
}

resource "aws_ssm_parameter" "ed25519_secret_key" {
  name             = "${var.config.name}-ed25519-secret-key"
  type             = "SecureString"
  value_wo         = var.secrets.ed25519_secret_key
  value_wo_version = parseint(substr(sha1(var.secrets.ed25519_secret_key), 0, 8), 16)
}

data "aws_region" "current" {}

module "vpc" {
  source  = "terraform-aws-modules/vpc/aws"
  version = "~> 6.5"

  name = var.config.name
  cidr = "10.0.0.0/16"

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
    secrets_version = sha1({
      secret_key = var.secrets.ed25519_secret_key
    })
  })
}
