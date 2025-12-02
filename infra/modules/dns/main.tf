terraform {
  required_providers {
    aws = {
      source  = "hashicorp/aws"
    }
    cloudflare = {
      source = "cloudflare/cloudflare"
    }
  }
}

variable "config" {
  type = object({
    domain_name = string
    cloudflare_zone_id = string
  })
}

resource "aws_route53_zone" "this" {
  name     = var.config.domain_name
}

resource "cloudflare_dns_record" "ns_delegation" {
  count = 4
  zone_id = var.config.cloudflare_zone_id
  name    = var.config.domain_name
  content = aws_route53_zone.this.name_servers[count.index]
  type    = "NS"
  ttl     = 1
}

resource "aws_acm_certificate" "this" {
  domain_name               = var.config.domain_name
  validation_method         = "DNS"

  lifecycle {
    create_before_destroy = true
  }
}

locals {
  domain_validation = tolist(aws_acm_certificate.this.domain_validation_options)[0]
}

resource "aws_route53_record" "cert_verification" {
  zone_id = aws_route53_zone.this.zone_id
  name    = local.domain_validation.resource_record_name
  type    = local.domain_validation.resource_record_type
  records = [local.domain_validation.resource_record_value]
  ttl     = 300

  allow_overwrite = true
}

resource "aws_acm_certificate_validation" "this" {
  certificate_arn         = aws_acm_certificate.this.arn
  validation_record_fqdns = [aws_route53_record.cert_verification.fqdn]
}
