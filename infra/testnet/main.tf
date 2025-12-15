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
  # default_tags {
  #   tags = local.tags
  # }
}

provider "aws" {
  region = "eu-central-1"
  alias  = "eu"
  # default_tags {
  #   tags = local.tags
  # }
}

provider "cloudflare" {
  api_token = var.cloudflare_wcf_api_token
}

provider "sops" {}

module "sops-encryption-key" {
  source = "../modules/sops-encryption-key"
}

resource "aws_route53_zone" "this" {
  name = "testnet.walletconnect.network"
}

resource "cloudflare_dns_record" "ns_delegation" {
  count   = 4
  zone_id = "a97af2cd2fd2da7a93413e455ed47f2c"
  name    = aws_route53_zone.this.name
  content = aws_route53_zone.this.name_servers[count.index]
  type    = "NS"
  ttl     = 1
}

locals {
  tags = {
    Application = "wcn2"
  }

  db_config = {
    image     = "ghcr.io/walletconnect/wcn-db:251113.0"
    cpu_cores = 2
    memory    = 4
    disk      = 50
  }

  node_config = {
    image     = "ghcr.io/walletconnect/wcn-node:251113.0"
    cpu_cores = 1
    memory    = 2
  }

  prometheus_config = {
    image     = "docker.io/prom/prometheus:v3.7.3"
    cpu_burst = true
    cpu_cores = 2
    memory    = 4
    disk      = 20
  }

  grafana_config = {
    image     = "docker.io/grafana/grafana:12.3"
    cpu_burst = true
    cpu_cores = 2
    memory    = 1
    disk      = 5

    prometheus_regions = ["eu"]
  }

  eu_smart_contract_address = "0x31551311408e4428b82e1acf042217a5446ff490"

  eu_operators = {
    wallet-connect = {
      vpc_cidr_octet = 105 # 10.105.0.0/16
      # For this one operator use x86 box instead of the default ARM,
      # so we have both architectures being actively tested.
      db = merge(local.db_config, { cpu_arch = "x86" })
      nodes = [
        local.node_config,
        local.node_config,
      ]
      prometheus   = local.prometheus_config
      grafana      = local.grafana_config
      route53_zone = aws_route53_zone.this
    }

    operator-a = {
      vpc_cidr_octet = 0 # 10.0.0.0/16
      db             = local.db_config
      nodes = [
        local.node_config,
        local.node_config,
      ]
    }

    operator-b = {
      vpc_cidr_octet = 0 # 10.0.0.0/16
      db             = local.db_config
      nodes = [
        local.node_config,
        local.node_config,
      ]
    }

    operator-c = {
      vpc_cidr_octet = 0 # 10.0.0.0/16
      db             = local.db_config
      nodes = [
        local.node_config,
        local.node_config,
      ]
    }

    operator-d = {
      vpc_cidr_octet = 0 # 10.0.0.0/16
      db             = local.db_config
      nodes = [
        local.node_config,
        local.node_config,
      ]
      create_ec2_instance_connect_endpoint = false
    }

    # Uncomment to deploy an extra operator and test migrations.
    # The SOPS file for this operator already exists.

    # operator-e = {
    #   vpc_cidr_octet = 0 # 10.0.0.0/16
    #   db             = local.db_config
    #   nodes = [
    #     local.node_config,
    #     local.node_config,
    #   ]
    # }
  }
}

module "eu-central-1" {
  source   = "../modules/node-operator"
  for_each = local.eu_operators

  config = merge(each.value, {
    name                   = each.key
    smart_contract_address = local.eu_smart_contract_address
    secrets_file_path      = "${path.module}/secrets/${each.key}.sops.json"
  })

  providers = {
    aws = aws.eu
  }
}

output "sops-encryption-key-arn" {
  value = module.sops-encryption-key.arn
}
