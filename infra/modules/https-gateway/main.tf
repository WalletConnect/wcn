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

resource "aws_security_group" "this" {
  name   = local.name
  vpc_id = var.vpc.vpc_id

  ingress {
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  ingress {
    from_port   = 80
    to_port     = 80
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }
}

resource "aws_lb" "this" {
  name               = local.name
  internal           = false
  load_balancer_type = "application"
  security_groups    = [aws_security_group.this.id]
  subnets            = var.vpc.public_subnets

  enable_deletion_protection = false
}

resource "aws_lb_target_group" "this" {
  name        = var.service.name
  port        = var.service.port
  protocol    = "HTTP"
  vpc_id      = var.vpc.vpc_id
  target_type = "ip"
  health_check {
    enabled = false
  }
}

resource "aws_lb_target_group_attachment" "this" {
  target_group_arn = aws_lb_target_group.this.arn
  target_id        = var.service.ip
  port             = var.service.port
}

resource "aws_lb_listener" "http" {
  load_balancer_arn = aws_lb.this.arn
  port              = 80
  protocol          = "HTTP"

  default_action {
    type = "redirect"

    redirect {
      port        = "443"
      protocol    = "HTTPS"
      status_code = "HTTP_301"
    }
  }
}

resource "aws_lb_listener" "https" {
  load_balancer_arn = aws_lb.this.arn
  port              = 443
  protocol          = "HTTPS"
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
