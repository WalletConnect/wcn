terraform {
  required_providers {
    aws = {
      source = "hashicorp/aws"
    }
    cloudflare = {
      source = "cloudflare/cloudflare"
    }
    sops = {
      source = "carlpett/sops"
    }
  }
}

variable "config" {
  type = object({
    name                   = string
    domain_name            = optional(string)
    secrets_file_path      = string
    smart_contract_address = string

    vpc_cidr_octet = number

    db = object({
      image     = string
      cpu_burst = bool
      cpu       = number
      memory    = number
      disk      = number
    })

    nodes = list(object({
      image     = string
      cpu_burst = bool
      cpu       = number
      memory    = number
    }))

    prometheus = optional(object({
      image     = string
      cpu_burst = bool
      cpu       = number
      memory    = number
      disk      = number
    }))

    grafana = optional(object({
      image     = string
      cpu_burst = bool
      cpu       = number
      memory    = number
      disk      = number

      prometheus_regions = list(string)
    }))

    dns = optional(object({
      domain_name        = string
      cloudflare_zone_id = string
    }))
  })
}

data "aws_region" "current" {}

ephemeral "sops_file" "secrets" {
  source_file = var.config.secrets_file_path
}

locals {
  octet         = var.config.vpc_cidr_octet
  region        = data.aws_region.current.region
  region_prefix = split("-", local.region)[0]

  encrypted_secrets = jsondecode(file(var.config.secrets_file_path))
  peer_id           = local.encrypted_secrets.peer_id_unencrypted

  # The decrypted secrets are not being stored in the TF state as they are `ephemeral`.
  secrets = jsondecode(ephemeral.sops_file.secrets.raw)
}

module "secret" {
  source = "../secret"
  for_each = toset(concat(
    ["ecdsa_private_key", "ed25519_secret_key", "smart_contract_encryption_key", "rpc_provider_url"],
    var.config.prometheus == null ? [] : ["prometheus_grafana_password"],
    var.config.grafana == null ? [] : ["grafana_admin_password"],
  ))

  name            = "${var.config.name}-${each.key}"
  ephemeral_value = local.secrets[each.key]
  value           = local.encrypted_secrets[each.key]
}

module "vpc" {
  source  = "terraform-aws-modules/vpc/aws"
  version = "~> 6.5"

  name = var.config.name
  cidr = "10.${local.octet}.0.0/16"

  enable_nat_gateway = true
  single_nat_gateway = true

  azs             = ["${local.region}a", "${local.region}b"]
  private_subnets = ["10.${local.octet}.1.0/24"]
  public_subnets  = ["10.${local.octet}.101.0/24", "10.${local.octet}.102.0/24"]
}

locals {
  db_primary_rpc_server_port   = 3000
  db_secondary_rpc_server_port = 3001
  db_metrics_server_port       = 3002
}

module "db" {
  source = "../service"
  config = merge(var.config.db, {
    name = "${var.config.name}-db"

    vpc    = module.vpc
    subnet = module.vpc.private_subnet_objects[0]

    public_ip = false

    ports = [
      { port = local.db_primary_rpc_server_port, protocol = "udp", internal = true },
      { port = local.db_secondary_rpc_server_port, protocol = "udp", internal = true },
      { port = local.db_metrics_server_port, protocol = "tcp", internal = true },
    ]

    environment = {
      PRIMARY_RPC_SERVER_PORT   = tostring(local.db_primary_rpc_server_port)
      SECONDARY_RPC_SERVER_PORT = tostring(local.db_secondary_rpc_server_port)
      METRICS_SERVER_PORT       = tostring(local.db_metrics_server_port)
      ROCKSDB_DIR               = "/data"
    }

    secrets = {
      SECRET_KEY = module.secret["ed25519_secret_key"]
    }
  })
}

locals {
  node_primary_rpc_server_port   = 3010
  node_secondary_rpc_server_port = 3011
  node_metrics_server_port       = 3012
}

module "node" {
  source = "../service"
  count  = length(var.config.nodes)
  config = merge(var.config.nodes[count.index], {
    name = "${var.config.name}-node-${count.index + 1}"

    vpc    = module.vpc
    subnet = module.vpc.public_subnet_objects[count.index % length(module.vpc.public_subnet_objects)]

    public_ip = true

    ports = [
      { port = local.node_primary_rpc_server_port, protocol = "udp", internal = false },
      { port = local.node_secondary_rpc_server_port, protocol = "udp", internal = false },
      { port = local.node_metrics_server_port, protocol = "tcp", internal = true },
    ]

    environment = {
      PRIMARY_RPC_SERVER_PORT            = tostring(local.node_primary_rpc_server_port)
      SECONDARY_RPC_SERVER_PORT          = tostring(local.node_secondary_rpc_server_port)
      METRICS_SERVER_PORT                = tostring(local.node_metrics_server_port)
      DATABASE_RPC_SERVER_ADDRESS        = module.db.private_ip
      DATABASE_PEER_ID                   = local.peer_id
      DATABASE_PRIMARY_RPC_SERVER_PORT   = tostring(local.db_primary_rpc_server_port)
      DATABASE_SECONDARY_RPC_SERVER_PORT = tostring(local.db_secondary_rpc_server_port)
      SMART_CONTRACT_ADDRESS             = var.config.smart_contract_address
    }

    secrets = {
      SECRET_KEY                        = module.secret["ed25519_secret_key"]
      SMART_CONTRACT_SIGNER_PRIVATE_KEY = module.secret["ecdsa_private_key"]
      SMART_CONTRACT_ENCRYPTION_KEY     = module.secret["smart_contract_encryption_key"]
      RPC_PROVIDER_URL                  = module.secret["rpc_provider_url"]
    }
  })
}

locals {
  prometheus_port        = 3000
  prometheus_domain_name = try("prometheus.${local.region_prefix}.${var.config.dns.domain_name}", null)
}

module "prometheus_config" {
  source = "../secret"
  count  = var.config.prometheus == null ? 0 : 1
  name   = "${var.config.name}-prometheus-config"
  value = yamlencode({
    scrape_configs = [{
      job_name                 = local.region_prefix
      scrape_interval          = "1m"
      scrape_timeout           = "1m"
      fallback_scrape_protocol = "PrometheusText1.0.0"
      metrics_path             = "/metrics/cluster"
      static_configs = [{
        targets = ["${module.node[0].private_ip}:${local.node_metrics_server_port}"]
      }]
    }]
  })
}

module "prometheus_web_config" {
  source = "../secret-template"
  count  = var.config.prometheus == null ? 0 : 1
  name   = "${var.config.name}-prometheus-web-config"
  template = yamlencode({
    basic_auth_users = {
      grafana = "$${grafana_password_hash}"
    }
  })
  ephemeral_args = {
    grafana_password_hash = local.secrets["prometheus_grafana_password_hash"]
  }
  args = {
    grafana_password_hash = local.encrypted_secrets["prometheus_grafana_password_hash"]
  }
}

module "prometheus" {
  source = "../service"
  count  = var.config.prometheus != null ? 1 : 0
  config = merge(var.config.prometheus, {
    name = "${var.config.name}-prometheus"

    vpc    = module.vpc
    subnet = module.vpc.private_subnet_objects[0]

    public_ip = false

    ports = [
      { port = local.prometheus_port, protocol = "tcp", internal = true },
    ]

    environment = {}
    secrets = {
      CONFIG     = module.prometheus_config[0]
      WEB_CONFIG = module.prometheus_web_config[0]
    }

    entry_point = ["/bin/sh", "-c"]
    command = [<<-CMD
      printenv CONFIG > /tmp/prometheus.yml && \
      printenv WEB_CONFIG > /tmp/web.yml && \
      exec /bin/prometheus \
      --config.file=/tmp/prometheus.yml \
      --web.config.file=/tmp/web.yml \
      --web.listen-address=:${local.prometheus_port} \
      --storage.tsdb.path=/data
    CMD
    ]
  })
}

locals {
  grafana_port                          = 9090
  grafana_domain_name                   = try("grafana.${var.config.dns.domain_name}", null)
  grafana_prometheus_password_file_path = "/tmp/prometheus_password"
}

module "grafana_prometheus_datasource_config" {
  source = "../secret-template"
  count  = var.config.grafana == null ? 0 : 1
  name   = "${var.config.name}-grafana-prometheus-ds-config"
  template = yamlencode({
    apiVersion : 1
    datasources : [for region in var.config.grafana.prometheus_regions : {
      uid           = "prometheus-${region}"
      name          = "Prometheus (${region})"
      type          = "prometheus"
      access        = "proxy"
      url           = "https://prometheus.${region}.${var.config.dns.domain_name}"
      basicAuth     = true
      basicAuthUser = "grafana"
      secureJsonData = {
        basicAuthPassword = "$${prometheus_password}"
      }
    }]
  })
  ephemeral_args = {
    prometheus_password = local.secrets["prometheus_grafana_password"]
  }
  args = {
    prometheus_password = local.encrypted_secrets["prometheus_grafana_password"]
  }
}

module "grafana" {
  source = "../service"
  count  = var.config.grafana != null ? 1 : 0
  config = merge(var.config.grafana, {
    name = "${var.config.name}-grafana"

    vpc    = module.vpc
    subnet = module.vpc.private_subnet_objects[0]

    public_ip = false

    ports = [
      { port = local.grafana_port, protocol = "tcp", internal = true },
    ]

    environment = {
      GF_SERVER_HTTP_PORT    = tostring(local.grafana_port)
      GF_PATHS_DATA          = "/data"
      GF_SECURITY_ADMIN_USER = "admin"
    }

    secrets = {
      GF_SECURITY_ADMIN_PASSWORD   = module.secret["grafana_admin_password"]
      PROMETHEUS_DATASOURCE_CONFIG = module.grafana_prometheus_datasource_config[0]
    }

    entry_point = ["/bin/sh", "-c"]
    command = [<<-CMD
      printenv PROMETHEUS_DATASOURCE_CONFIG > /etc/grafana/provisioning/datasources/prometheus.yaml && \
      exec /run.sh
    CMD
    ]
  })
}

resource "aws_route53_zone" "this" {
  count = var.config.dns != null ? 1 : 0
  name  = var.config.dns.domain_name
}

resource "cloudflare_dns_record" "ns_delegation" {
  count   = var.config.dns != null ? 4 : 0
  zone_id = var.config.dns.cloudflare_zone_id
  name    = var.config.dns.domain_name
  content = aws_route53_zone.this[0].name_servers[count.index]
  type    = "NS"
  ttl     = 1
}

module "ssl_certificate" {
  source = "../ssl-certificate"
  for_each = toset(var.config.dns == null ? [] : concat(
    var.config.prometheus == null ? [] : [local.prometheus_domain_name],
    var.config.grafana == null ? [] : [local.grafana_domain_name],
  ))
  domain_name  = each.key
  route53_zone = aws_route53_zone.this[0]
}

module "prometheus_https_gateway" {
  source = "../https-gateway"
  count  = var.config.prometheus != null ? 1 : 0
  service = {
    name = "${var.config.name}-prom"
    ip   = module.prometheus[0].private_ip
    port = local.prometheus_port
  }
  vpc             = module.vpc
  certificate_arn = module.ssl_certificate[local.prometheus_domain_name].arn
}

module "grafana_https_gateway" {
  source = "../https-gateway"
  count  = var.config.grafana != null ? 1 : 0
  service = {
    name = "${var.config.name}-grafana"
    ip   = module.grafana[0].private_ip
    port = local.grafana_port
  }
  vpc             = module.vpc
  certificate_arn = module.ssl_certificate[local.grafana_domain_name].arn
}

resource "aws_route53_record" "prometheus" {
  count   = var.config.prometheus != null ? 1 : 0
  zone_id = aws_route53_zone.this[0].zone_id
  name    = local.prometheus_domain_name
  type    = "A"

  alias {
    name                   = module.prometheus_https_gateway[0].lb.dns_name
    zone_id                = module.prometheus_https_gateway[0].lb.zone_id
    evaluate_target_health = false
  }
}

resource "aws_route53_record" "grafana" {
  count   = var.config.grafana != null ? 1 : 0
  zone_id = aws_route53_zone.this[0].zone_id
  name    = local.grafana_domain_name
  type    = "A"

  alias {
    name                   = module.grafana_https_gateway[0].lb.dns_name
    zone_id                = module.grafana_https_gateway[0].lb.zone_id
    evaluate_target_health = false
  }
}

resource "aws_security_group" "ec2_instance_connect_endpoint" {
  name   = "${var.config.name}-ec2-instance-connect-endpoint"
  vpc_id = module.vpc.vpc_id

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = [module.vpc.vpc_cidr_block]
  }
}

resource "aws_ec2_instance_connect_endpoint" "this" {
  subnet_id          = module.vpc.private_subnet_objects[0].id
  security_group_ids = [aws_security_group.ec2_instance_connect_endpoint.id]
  preserve_client_ip = false
}
