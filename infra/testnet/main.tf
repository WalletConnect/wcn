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
    ec2_instance_type = "a1.large"

    ebs_volume_size = 50 # GiB
    ecs_task_container_image = "ghcr.io/walletconnect/wcn-db:f5867045-arm64"
    ecs_task_cpu             = 2048
    # 512 MiB are system reserved
    ecs_task_memory = 4096 - 512
  }

  eu_operators = {
    wallet-connect-1 = {
      db = local.db_config
    }
  }
}

module "eu-central-1" {
  source = "../modules/node-operator"
  for_each = local.eu_operators

  config = {
    name    = each.key
    secrets_file_path = "${path.module}/secrets/${each.key}.sops.yaml"

    db      = each.value.db
  }

  providers = {
    aws = aws.eu
  }
}

output "sops-encryption-key-arn" {
  value = module.sops-encryption-key.arn
}
