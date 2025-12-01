terraform {
  required_version = ">= 1.12"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
    cloudinit = {
      source  = "hashicorp/cloudinit"
      version = "~> 2.0"
    }
    sops = {
      source  = "carlpett/sops"
      version = "~> 1.3"
    }
  }
}

provider "aws" {
  region = "eu-central-1"
}

provider "aws" {
  region = "eu-central-1"
  alias = "eu"
}

provider "sops" {}

module "sops-encryption-key" {
  source = "../modules/sops-encryption-key"
}

locals {  
  db_config = {
    # 2 vCPU / 4 GiB RAM, arm64
    ec2_instance_type = "c6g.large"

    ebs_volume_size = 50 # GiB
    ecs_task_container_image = "ghcr.io/walletconnect/wcn-db:251113.0"
    ecs_task_cpu             = 2048
    # 512 MiB are system reserved
    ecs_task_memory = 4096 - 512
  }

  node_config = {
    # 1 vCPU / 2 GiB RAM, arm64
    ec2_instance_type = "c6g.medium"

    ecs_task_container_image = "ghcr.io/walletconnect/wcn-node:251113.0"
    ecs_task_cpu             = 1024
    # 512 MiB are system reserved
    ecs_task_memory = 2048 - 512
  }


  # 512 MiB are system reserved
  monitoring_config = {
    # 1 vCPU / 2 GiB RAM, arm64
    ec2_instance_type = "c6g.medium"

    ebs_volume_size = 20 # GiB

    prometheus = {
      ecs_task_container_image = "docker.io/prom/prometheus:v3.7.3"
      ecs_task_cpu             = 512
      ecs_task_memory = 1024
    }

    grafana = {
      ecs_task_container_image = "docker.io/grafana/grafana:12.3"
      ecs_task_cpu             = 512
      ecs_task_memory = 512
    }

    domain_name = "testnet.walletconnect.network"
    cloudflare_zone_id = "tbd"
  }

  eu_smart_contract_address = "0x31551311408e4428b82e1acf042217a5446ff490"

  eu_operators = {
    wallet-connect = {
      vpc_cidr_octet = 105 # 10.105.0.0/16
      db = local.db_config
      nodes = [
        local.node_config,
        local.node_config,
      ]
      monitoring = local.monitoring_config
    }

    operator-a = {
      vpc_cidr_octet = 0 # 10.0.0.0/16
      db = local.db_config
      nodes = [
        local.node_config,
        local.node_config,
      ]
    }

    operator-b = {
      vpc_cidr_octet = 0 # 10.0.0.0/16
      db = local.db_config
      nodes = [
        local.node_config,
        local.node_config,
      ]
    }

    operator-c = {
      vpc_cidr_octet = 0 # 10.0.0.0/16
      db = local.db_config
      nodes = [
        local.node_config,
        local.node_config,
      ]
    }

    operator-d = {
      vpc_cidr_octet = 0 # 10.0.0.0/16
      db = local.db_config
      nodes = [
        local.node_config,
        local.node_config,
      ]
    }
  }
}

module "eu-central-1" {
  source = "../modules/node-operator"
  for_each = local.eu_operators

  config = merge(each.value, {
    name    = each.key
    smart_contract_address = local.eu_smart_contract_address
    secrets_file_path = "${path.module}/secrets/${each.key}.sops.json"
  })

  providers = {
    aws = aws.eu
  }
}

output "sops-encryption-key-arn" {
  value = module.sops-encryption-key.arn
}
