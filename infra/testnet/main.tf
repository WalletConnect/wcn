terraform {
  required_version = ">= 1.12"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
    sops = {
      source = "carlpett/sops"
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

data "sops_file" "test" {
  source_file = "test.yaml"
}

output "sops-encryption-key-arn" {
  value = module.sops-encryption-key.arn
}
