terraform {
  required_version = ">= 1.12"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = ">= 6.0"
    }
  }
}

module "sops-encryption-key" {
  source = "../modules/sops-encryption-key"
}
