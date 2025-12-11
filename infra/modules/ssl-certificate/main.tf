variable "domain_name" {
  type = string
}

variable "route53_zone" {
  type = object({
    zone_id = string
  })
}

resource "aws_acm_certificate" "this" {
  domain_name       = var.domain_name
  validation_method = "DNS"

  lifecycle {
    create_before_destroy = true
  }
}

locals {
  domain_validation = tolist(aws_acm_certificate.this.domain_validation_options)[0]
}

resource "aws_route53_record" "cert_verification" {
  zone_id = var.route53_zone.zone_id
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

output "arn" {
  value = aws_acm_certificate.this.arn
}
