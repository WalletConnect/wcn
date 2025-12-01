terraform {
  required_providers {
    aws = {
      source  = "hashicorp/aws"
    }
    sops = {
      source  = "carlpett/sops"
    }
  }
}

variable "config" {
  type = object({
    name    = string
    secrets_file_path = string
    smart_contract_address = string

    vpc_cidr_octet = number
    
    db = object({
      ec2_instance_type = string

      ebs_volume_size = number

      ecs_task_container_image = string
      ecs_task_cpu             = number
      ecs_task_memory          = number
    })

    nodes = list(object({
      ec2_instance_type = string

      ecs_task_container_image = string
      ecs_task_cpu             = number
      ecs_task_memory          = number
    }))

    monitoring = optional(object({
      ec2_instance_type = string

      ebs_volume_size = number

      prometheus = object({
        ecs_task_container_image = string
        ecs_task_cpu             = number
        ecs_task_memory          = number
      })

      grafana = object({
        ecs_task_container_image = string
        ecs_task_cpu             = number
        ecs_task_memory          = number
      })
    }))

    dns = optional(object({
      domain_name = string
      cloudflare_zone_id = string
    }))
  })
}

data "aws_region" "current" {}

ephemeral "sops_file" "secrets" {
  source_file = var.config.secrets_file_path
}

locals {
  octet = var.config.vpc_cidr_octet
  region = data.aws_region.current.region

  # We store encrypted secrets as a `local` to be able to derive secret versions.
  encrypted_secrets = {
    for k, v in jsondecode(file(var.config.secrets_file_path)):
    # Remove SOPS metadata
    k => v if k != "sops"
  }

  # The decrypted secrets are not being stored in the TF state as they are `ephemeral`.
  secrets = jsondecode(ephemeral.sops_file.secrets.raw)

  peer_id = local.encrypted_secrets.peer_id_unencrypted

  create_monitoring = try(var.config.monitoring, null) != null 
}

# module "secret" {
#   source = "../secret"
#   for_each = local.encrypted_secrets

#   name = "test-${var.config.name}-ed25519-secret-key"
#   value = local.secrets[each.key]
#   value_encrypted = each.value
# }

resource "aws_ssm_parameter" "ed25519_secret_key" {
  name             = "${var.config.name}-ed25519-secret-key"
  type             = "SecureString"
  value_wo         = local.secrets.ed25519_secret_key
  value_wo_version = parseint(substr(sha1(local.encrypted_secrets.ed25519_secret_key), 0, 8), 16)
}

resource "aws_ssm_parameter" "ecdsa_private_key" {
  name             = "${var.config.name}-ecdsa-private-key"
  type             = "SecureString"
  value_wo         = local.secrets.ecdsa_private_key
  value_wo_version = parseint(substr(sha1(local.encrypted_secrets.ecdsa_private_key), 0, 8), 16)
}

resource "aws_ssm_parameter" "smart_contract_encryption_key" {
  name             = "${var.config.name}-smart-contract-encryption-key"
  type             = "SecureString"
  value_wo         = local.secrets.smart_contract_encryption_key
  value_wo_version = parseint(substr(sha1(local.encrypted_secrets.smart_contract_encryption_key), 0, 8), 16)
}

resource "aws_ssm_parameter" "rpc_provider_url" {
  name             = "${var.config.name}-rpc-provider-url"
  type             = "SecureString"
  value_wo         = local.secrets.rpc_provider_url
  value_wo_version = parseint(substr(sha1(local.encrypted_secrets.rpc_provider_url), 0, 8), 16)
}

resource "aws_ssm_parameter" "grafana_admin_password" {
  count = local.create_monitoring ? 1 : 0
  
  name             = "${var.config.name}-grafana-admin-password"
  type             = "SecureString"
  value_wo         = local.secrets.grafana_admin_password
  value_wo_version = parseint(substr(sha1(local.encrypted_secrets.grafana_admin_password), 0, 8), 16)
}

module "vpc" {
  source  = "terraform-aws-modules/vpc/aws"
  version = "~> 6.5"

  name = var.config.name
  cidr = "10.${local.octet}.0.0/16"

  enable_nat_gateway = true
  single_nat_gateway = true
  
  azs             = ["${local.region}a", "${local.region}b"]
  private_subnets = ["10.${local.octet}.1.0/24"]
  public_subnets  = ["10.${local.octet}.101.0/24", "10.${local.octet}.102.0/24"]
}

module "db" {
  source = "../db"
  config = merge(var.config.db, {
    operator_name = var.config.name

    vpc    = module.vpc
    subnet = module.vpc.private_subnet_objects[0]

    primary_rpc_server_port   = 3000
    secondary_rpc_server_port = 3001
    metrics_server_port       = 3002

    secret_key_arn = aws_ssm_parameter.ed25519_secret_key.arn
    secrets_version = sha1(jsonencode([
      local.encrypted_secrets.ed25519_secret_key,
    ]))
  })
}

module "node" {
  source = "../node"
  count = length(var.config.nodes)
  config = merge(var.config.nodes[count.index], {
    index = count.index
    operator_name = var.config.name

    vpc    = module.vpc
    subnet = module.vpc.public_subnet_objects[length(module.vpc.public_subnet_objects) % length(var.config.nodes)]

    primary_rpc_server_port   = 3010
    secondary_rpc_server_port = 3011
    metrics_server_port       = 3012

    database_rpc_server_address = module.db.rpc_server_address
    database_peer_id = local.peer_id
    database_primary_rpc_server_port = module.db.primary_rpc_server_port
    database_secondary_rpc_server_port = module.db.secondary_rpc_server_port

    smart_contract_address = var.config.smart_contract_address

    secret_key_arn = aws_ssm_parameter.ed25519_secret_key.arn
    smart_contract_signer_private_key_arn = aws_ssm_parameter.ecdsa_private_key.arn
    smart_contract_encryption_key_arn = aws_ssm_parameter.smart_contract_encryption_key.arn
    rpc_provider_url_arn = aws_ssm_parameter.rpc_provider_url.arn
    secrets_version = sha1(jsonencode([
      local.encrypted_secrets.ed25519_secret_key,
      local.encrypted_secrets.ecdsa_private_key,
      local.encrypted_secrets.smart_contract_encryption_key,
      local.encrypted_secrets.rpc_provider_url,
    ]))
  })
}

module "monitoring" {
  source = "../monitoring"
  count = local.create_monitoring ? 1 : 0

  config = merge(var.config.monitoring, {
    operator_name = var.config.name

    vpc    = module.vpc
    subnet = module.vpc.private_subnet_objects[0]

    grafana = merge(var.config.monitoring.grafana, {
      admin_password_arn = aws_ssm_parameter.grafana_admin_password[0].arn
      secrets_version = sha1(jsonencode([
        local.encrypted_secrets.grafana_admin_password,
      ]))
    })
  })
}

# resource "aws_acm_certificate" "this" {
#   domain_name               = var.config.hosted_zone.name
#   validation_method         = "DNS"

#   lifecycle {
#     create_before_destroy = true
#   }
# }

# resource "aws_route53_record" "grafana" {
#   zone_id = var.hosted_zone.zone_id
#   name    = var.load_balancers[count.index].name
#   type    = "A"

#   alias {
#     name                   = var.load_balancers[count.index].dns_name
#     zone_id                = var.load_balancers[count.index].zone_id
#     evaluate_target_health = true
#   }
# }

resource "aws_security_group" "ec2_instance_connect_endpoint" {
  name   = "${var.config.name}-ec2-instance-connect-endpoint"
  vpc_id = module.vpc.vpc_id

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = [module.vpc.vpc_cidr_block]
  }
}

resource "aws_ec2_instance_connect_endpoint" "this" {
  subnet_id = module.vpc.private_subnet_objects[0].id
  security_group_ids = [aws_security_group.ec2_instance_connect_endpoint.id]
  preserve_client_ip = false
}
