variable "service" {
  type = object({
    name = string
    ip   = string
    port = number
  })
}

variable "vpc" {
  type = object({
    vpc_id         = string
    public_subnets = list(string)
  })
}

variable "certificate_arn" {
  type = string
}

locals {
  name = "${var.service.name}-https-gw"
}

resource "aws_lb" "this" {
  name               = local.name
  internal           = false
  load_balancer_type = "network"
  subnets            = var.vpc.public_subnets

  enable_deletion_protection = false
}

resource "aws_lb_target_group" "this" {
  name        = var.service.name
  port        = var.service.port
  protocol    = "TCP"
  vpc_id      = var.vpc.vpc_id
  target_type = "ip"
}

resource "aws_lb_target_group_attachment" "this" {
  target_group_arn = aws_lb_target_group.this.arn
  target_id        = var.service.ip
  port             = var.service.port
}

resource "aws_lb_listener" "https" {
  load_balancer_arn = aws_lb.this.arn
  port              = 443
  protocol          = "TLS"
  ssl_policy        = "ELBSecurityPolicy-TLS13-1-2-Res-PQ-2025-09"
  certificate_arn   = var.certificate_arn

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.this.arn
  }
}

output "lb" {
  value = aws_lb.this
}
