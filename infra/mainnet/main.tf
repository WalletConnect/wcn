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

output "sops-encryption-key-arn" {
  value = module.sops-encryption-key.arn
}
