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
  type      = string
  sensitive = true
  ephemeral = true
}

provider "aws" {
  region = "eu-central-1"
}

provider "aws" {
  region = "eu-central-1"
  alias  = "eu"
}

provider "aws" {
  region = "us-east-1"
  alias  = "us"
}

provider "aws" {
  region = "ap-southeast-1"
  alias  = "ap"
}

provider "aws" {
  region = "sa-east-1"
  alias  = "sa"
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
    image     = "ghcr.io/walletconnect/wcn-db:251113.0"
    cpu_arch  = "x86"
    cpu_cures = 8
    memory    = 16
    disk      = 100
  }

  node_config = {
    image     = "ghcr.io/walletconnect/wcn-node:251113.0"
    cpu_cores = 4
    memory    = 8
  }

  prometheus_config = {
    image     = "docker.io/prom/prometheus:v3.7.3"
    cpu_burst = true
    cpu_cores = 2
    memory    = 8
    disk      = 50
  }

  grafana_config = {
    image     = "docker.io/grafana/grafana:12.3"
    cpu_burst = true
    cpu_cores = 2
    memory    = 4
    disk      = 5

    prometheus_regions = ["eu", "us", "ap", "sa"]
  }
}

module "wallet-connect-eu" {
  source = "../modules/node-operator"

  config = {
    name                   = "wallet-connect"
    secrets_file_path      = "${path.module}/secrets/wallet-connect-eu.sops.json"
    vpc_cidr_octet         = 5 # 10.5.0.0/16
    smart_contract_address = "0xa18770BFAb520CdD101680cCF3252D642713F3fC"
    db                     = local.db_config
    nodes = [
      local.node_config,
      local.node_config,
    ]
    # prometheus = local.prometheus_config
    # grafana    = local.grafana_config
    # dns = {
    #   domain_name        = "testnet.walletconnect.network"
    #   cloudflare_zone_id = "a97af2cd2fd2da7a93413e455ed47f2c"
    # }
  }

  providers = {
    aws = aws.eu
  }
}

module "wallet-connect-sa" {
  source = "../modules/node-operator"

  config = {
    name                   = "wallet-connect"
    secrets_file_path      = "${path.module}/secrets/wallet-connect-sa.sops.json"
    vpc_cidr_octet         = 8 # 10.8.0.0/16
    smart_contract_address = "0xca5b9bd2cf8045ff8308454c1b9caef2a6fcc20f"
    db                     = local.db_config
    nodes = [
      local.node_config,
      local.node_config,
    ]
  }

  providers = {
    aws = aws.sa
  }
}

output "sops-encryption-key-arn" {
  value = module.sops-encryption-key.arn
}
