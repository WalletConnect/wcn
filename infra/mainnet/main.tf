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

locals {
  aws_tags = {
    Application = "wcn"
  }
}

provider "aws" {
  region = "eu-central-1"
  default_tags {
    tags = local.aws_tags
  }
}

provider "aws" {
  region = "eu-central-1"
  alias  = "eu"
  default_tags {
    tags = local.aws_tags
  }
}

provider "aws" {
  region = "us-east-1"
  alias  = "us"
  default_tags {
    tags = local.aws_tags
  }
}

provider "aws" {
  region = "ap-southeast-1"
  alias  = "ap"
  default_tags {
    tags = local.aws_tags
  }
}

provider "aws" {
  region = "sa-east-1"
  alias  = "sa"
  default_tags {
    tags = local.aws_tags
  }
}

provider "cloudflare" {
  api_token = var.cloudflare_wcf_api_token
}

provider "sops" {}

module "sops-encryption-key" {
  source = "../modules/sops-encryption-key"
}

module "admin-key-eu" {
  source = "../modules/admin-key"
  providers = {
    aws = aws.eu
  }
}

module "admin-key-us" {
  source = "../modules/admin-key"
  providers = {
    aws = aws.us
  }
}

module "admin-key-ap" {
  source = "../modules/admin-key"
  providers = {
    aws = aws.us
  }
}

module "admin-key-sa" {
  source = "../modules/admin-key"
  providers = {
    aws = aws.us
  }
}

resource "aws_route53_zone" "this" {
  name = "mainnet.walletconnect.network"
}

resource "cloudflare_dns_record" "ns_delegation" {
  count   = 4
  zone_id = local.cloudflare_zone_id
  name    = aws_route53_zone.this.name
  content = aws_route53_zone.this.name_servers[count.index]
  type    = "NS"
  ttl     = 1
}

locals {
  cloudflare_zone_id = "a97af2cd2fd2da7a93413e455ed47f2c"

  db_config = {
    image     = "ghcr.io/walletconnect/wcn-db:251113.0"
    cpu_arch  = "x86"
    cpu_cores = 8
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

  relay_account_id = "780545098720"
  vpc_peering_connections = {
    "relay-eu-central-1" : {
      account_id = local.relay_account_id
      cidr       = "10.11.0.0/16"
    }
    "relay-us-east-1" : {
      account_id = local.relay_account_id
      cidr       = "10.12.0.0/16"
    }
    "relay-ap-southeast-1" : {
      account_id = local.relay_account_id
      cidr       = "10.13.0.0/16"
    }
    "relay-sa-east-1" : {
      account_id = local.relay_account_id
      cidr       = "10.14.0.0/16"
    }
  }

  eu_operators = {
    wallet-connect = {
      vpc_cidr_octet          = 5 # 10.5.0.0/16
      vpc_peering_connections = local.vpc_peering_connections
      db                      = local.db_config
      nodes = [
        local.node_config,
        local.node_config,
      ]
      prometheus   = local.prometheus_config
      grafana      = local.grafana_config
      route53_zone = aws_route53_zone.this
    }
    wallet-connect-2 = {
      vpc_cidr_octet = 0 # 10.0.0.0/16
      db             = local.db_config
      nodes = [
        local.node_config,
        local.node_config,
      ]
    }
    wallet-connect-3 = {
      vpc_cidr_octet = 0 # 10.0.0.0/16
      db             = local.db_config
      nodes = [
        local.node_config,
        local.node_config,
      ]
    }
    wallet-connect-4 = {
      vpc_cidr_octet = 0 # 10.0.0.0/16
      db             = local.db_config
      nodes = [
        local.node_config,
        local.node_config,
      ]
    }
    wallet-connect-5 = {
      vpc_cidr_octet = 0 # 10.0.0.0/16
      db             = local.db_config
      nodes = [
        local.node_config,
        local.node_config,
      ]
    }
  }

  us_operators = {
    wallet-connect = {
      vpc_cidr_octet          = 6 # 10.6.0.0/16
      vpc_peering_connections = local.vpc_peering_connections
      db                      = local.db_config
      nodes = [
        local.node_config,
        local.node_config,
      ]
      prometheus   = local.prometheus_config
      route53_zone = aws_route53_zone.this
    }
  }

  ap_operators = {
    wallet-connect = {
      vpc_cidr_octet          = 7 # 10.7.0.0/16
      vpc_peering_connections = local.vpc_peering_connections
      db                      = local.db_config
      nodes = [
        local.node_config,
        local.node_config,
      ]
      prometheus   = local.prometheus_config
      route53_zone = aws_route53_zone.this
    }
  }

  sa_operators = {
    wallet-connect = {
      vpc_cidr_octet          = 8 # 10.8.0.0/16
      vpc_peering_connections = local.vpc_peering_connections
      db                      = local.db_config
      nodes = [
        local.node_config,
        local.node_config,
      ]
      prometheus   = local.prometheus_config
      route53_zone = aws_route53_zone.this
    }
  }
}

module "eu-central-1" {
  source   = "../modules/node-operator"
  for_each = local.eu_operators

  config = merge(each.value, {
    name           = each.key
    sops_file_path = "${path.module}/sops/eu.${each.key}.json"
  })

  providers = {
    aws = aws.eu
  }
}

module "us-east-1" {
  source   = "../modules/node-operator"
  for_each = local.us_operators

  config = merge(each.value, {
    name           = each.key
    sops_file_path = "${path.module}/sops/us.${each.key}.json"
  })

  providers = {
    aws = aws.us
  }
}

module "ap-southeast-1" {
  source   = "../modules/node-operator"
  for_each = local.ap_operators

  config = merge(each.value, {
    name           = each.key
    sops_file_path = "${path.module}/sops/ap.${each.key}.json"
  })

  providers = {
    aws = aws.ap
  }
}

module "sa-east-1" {
  source   = "../modules/node-operator"
  for_each = local.sa_operators

  config = merge(each.value, {
    name           = each.key
    sops_file_path = "${path.module}/sops/sa.${each.key}.json"
  })

  providers = {
    aws = aws.sa
  }
}

output "sops-encryption-key-arn" {
  value = module.sops-encryption-key.arn
}
