terraform {
  required_version = ">= 1.12"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
    sops = {
      source  = "carlpett/sops"
      version = "~> 0.5"
    }
  }
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

module "sops-encryption-key" {
  source = "../modules/sops-encryption-key"
}

ephemeral "sops_file" "secrets" {
  source_file = "secrets.yaml"
}

locals {
  db_config = {
    # 2 vCPU / 4 GiB RAM, x86_64
    ec2_instance_type = "c5a.large"

    ebs_volume_size = 50 # GiB

    ecs_task_container_image = "ghcr.io/walletconnect/wcn-db:251024.0"
    ecs_task_cpu             = 2048
    # 512 MiB are system reserved
    ecs_task_memory = 4096 - 512
  }
}

module "eu-central-1-node-operator-1" {
  source = "../modules/node-operator"

  config = {
    name    = "wallet-connect-1"
    peer_id = "12D3KooWKpZ7ch5k4ggZHAKKMmu1oNnBxM5u1VZpGiopPZQmrLHT"
    db      = local.db_config
  }

  secrets = data.sops_file.secrets.data["operators"][0]

  providers = {
    aws = aws.eu
  }
}

output "sops-encryption-key-arn" {
  value = module.sops-encryption-key.arn
}
