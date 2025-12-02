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
      image = string
      cpu = number
      cpu_burst = bool
      memory = number
      disk = number
    })

    nodes = list(object({
      image = string
      cpu = number
      cpu_burst = bool
      memory = number
    }))

    prometheus = optional(object({
      image = string
      cpu = number
      cpu_burst = bool
      memory = number
      disk = number
    }))

    grafana = optional(object({
      image = string
      cpu = number
      cpu_burst = bool
      memory = number
      disk = number
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

  encrypted_secrets = jsondecode(file(var.config.secrets_file_path))
  peer_id = local.encrypted_secrets.peer_id_unencrypted

  # The decrypted secrets are not being stored in the TF state as they are `ephemeral`.
  secrets = jsondecode(ephemeral.sops_file.secrets.raw)
}

module "secret" {
  source = "../secret"
  for_each = {
    for k, v in local.encrypted_secrets:
    # Remove non-encrypted values and SOPS metadata
    k => v if !endswith(k, "_unencrypted") && k != "sops" 
  }

  name = "${var.config.name}-${each.key}"
  value = local.secrets[each.key]
  value_encrypted = each.value
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

locals {
  db_primary_rpc_server_port = 3000
  db_secondary_rpc_server_port = 3001
  db_metrics_server_port = 3002
}

module "db" {
  source = "../service"
  config = merge(var.config.db, {
    name = "${var.config.name}-db"

    vpc    = module.vpc
    subnet = module.vpc.private_subnet_objects[0]

    public_ip = false

    ports = [
      { port = local.db_primary_rpc_server_port, protocol = "udp", internal = true },
      { port = local.db_secondary_rpc_server_port, protocol = "udp", internal = true },
      { port = local.db_metrics_server_port, protocol = "tcp", internal = true },
    ]

    environment = {
      PRIMARY_RPC_SERVER_PORT = tostring(local.db_primary_rpc_server_port)
      SECONDARY_RPC_SERVER_PORT = tostring(local.db_secondary_rpc_server_port)
      METRICS_SERVER_PORT = tostring(local.db_metrics_server_port)
      ROCKSDB_DIR = "/data"
    }

    secrets = {
      SECRET_KEY = module.secret["ed25519_secret_key"]
    }
  })
}

locals {
  node_primary_rpc_server_port = 3010
  node_secondary_rpc_server_port = 3011
  node_metrics_server_port = 3012
}

module "node" {
  source = "../service"
  count = length(var.config.nodes)
  config = merge(var.config.nodes[count.index], {
    name = "${var.config.name}-node-${count.index + 1}"

    vpc    = module.vpc
    subnet = module.vpc.public_subnet_objects[count.index % length(module.vpc.public_subnet_objects)]

    public_ip = true

    ports = [
      { port = local.node_primary_rpc_server_port, protocol = "udp", internal = false },
      { port = local.node_secondary_rpc_server_port, protocol = "udp", internal = false },
      { port = local.node_metrics_server_port, protocol = "tcp", internal = true },
    ]

    environment = {
      PRIMARY_RPC_SERVER_PORT = tostring(local.node_primary_rpc_server_port)
      SECONDARY_RPC_SERVER_PORT = tostring(local.node_secondary_rpc_server_port)
      METRICS_SERVER_PORT = tostring(local.node_metrics_server_port)
      DATABASE_RPC_SERVER_ADDRESS = module.db.private_ip
      DATABASE_PEER_ID = local.peer_id
      DATABASE_PRIMARY_RPC_SERVER_PORT = tostring(local.db_primary_rpc_server_port)
      DATABASE_SECONDARY_RPC_SERVER_PORT = tostring(local.db_secondary_rpc_server_port)
      SMART_CONTRACT_ADDRESS = var.config.smart_contract_address
    }

    secrets = {
      SECRET_KEY = module.secret["ed25519_secret_key"]
      SMART_CONTRACT_SIGNER_PRIVATE_KEY = module.secret["ecdsa_private_key"]
      SMART_CONTRACT_ENCRYPTION_KEY = module.secret["smart_contract_encryption_key"]
      RPC_PROVIDER_URL = module.secret["rpc_provider_url"]
    }
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
