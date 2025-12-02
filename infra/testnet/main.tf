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
    cloudflare = {
      source  = "cloudflare/cloudflare"
      version = "~> 5.0"
    }
    sops = {
      source  = "carlpett/sops"
      version = "~> 1.3"
    }
  }
}

variable "cloudflare_wcf_api_token" {
  type = string
  sensitive = true
  ephemeral = true
}

provider "aws" {
  region = "eu-central-1"
}

provider "aws" {
  region = "eu-central-1"
  alias = "eu"
}

provider "cloudflare" {
  api_token = var.cloudflare_wcf_api_token
}

provider "sops" {}

module "sops-encryption-key" {
  source = "../modules/sops-encryption-key"
}

locals {  
  db_config = {
    image = "ghcr.io/walletconnect/wcn-db:251113.0"
    cpu_burst = false
    cpu = 2
    memory = 4
    disk = 50
  }

  node_config = {
    image = "ghcr.io/walletconnect/wcn-node:251113.0"
    cpu_burst = false
    cpu = 1
    memory = 2
  }

  prometheus_config = {
    image = "docker.io/prom/prometheus:v3.7.3"
    cpu_burst = true
    cpu = 2
    memory = 1
    disk = 20
  }

  grafana_config = {
    image = "docker.io/grafana/grafana:12.3"
    cpu_burst = true
    cpu = 2
    memory = 1
    disk = 5
  }

  eu_smart_contract_address = "0x31551311408e4428b82e1acf042217a5446ff490"

  eu_operators = {
    wallet-connect = {
      domain_name = "walletconnect.network"
      vpc_cidr_octet = 105 # 10.105.0.0/16
      db = local.db_config
      nodes = [
        local.node_config,
        local.node_config,
      ]
      prometheus = local.prometheus_config
      grafana = local.grafana_config
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

output "cf_zones" {
  value = module.eu-central-1["wallet-connect"].zones
}
