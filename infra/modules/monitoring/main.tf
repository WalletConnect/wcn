variable "config" {
  type = object({
    operator_name = string

    vpc = object({
      vpc_id         = string
      vpc_cidr_block = string
    })

    subnet = object({
      id                = string
      availability_zone = string
    })

    ec2_instance_type = string
    ebs_volume_size   = number

    prometheus = object({
      ecs_task_container_image = string
      ecs_task_cpu             = number
      ecs_task_memory          = number
    })

    grafana = object({
      ecs_task_container_image = string
      ecs_task_cpu             = number
      ecs_task_memory          = number

      admin_password_arn = string

      # Force ECS task update when secrets / config change
      secrets_version = string
    })
  })
}

data "aws_ssm_parameter" "ami_id" {
  name = "/aws/service/ecs/optimized-ami/amazon-linux-2023/arm64/al2023-ami-ecs-hvm-2023.0.20251108-kernel-6.1-arm64/image_id"
}

locals {
  name = "${var.config.operator_name}-monitoring"

  az     = var.config.subnet.availability_zone
  region = substr(local.az, 0, length(local.az) - 1)

  ebs_data_volume = {
    device = "/dev/xvdh"
    mount_point = "/mnt/data"
  }

  prometheus_docker_volume = {
    name = "prometheus-data"
    host_path = "${local.ebs_data_volume.mount_point}/prometheus"
    container_path = "data"
  }

  prometheus_port = 3000

  grafana_docker_volume = {
    name = "grafana-data"
    host_path = "${local.ebs_data_volume.mount_point}/grafana"
    container_path = "data"
  }

  grafana_port = 9090
}

resource "aws_security_group" "this" {
  name   = local.name
  vpc_id = var.config.vpc.vpc_id

  ingress {
    description = "Grafana HTTP UI"
    from_port   = local.grafana_port
    to_port     = local.grafana_port
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  ingress {
    description = "EC2 Instance Connect"
    from_port   = 22
    to_port     = 22
    protocol    = "tcp"
    cidr_blocks = [var.config.vpc.vpc_cidr_block]
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }
}

data "cloudinit_config" "this" {
  gzip = false

  part {
    filename     = "userdata.sh"
    content_type = "text/x-shellscript"
    content = templatefile("${path.module}/userdata.sh", {
      ecs_cluster     = aws_ecs_cluster.this.name
      ebs_device_path = local.ebs_data_volume.device
      mount_point     = local.ebs_data_volume.mount_point
    })
  }
}

resource "terraform_data" "userdata_fingerprint" {
  input = sha256(data.cloudinit_config.this.rendered)
}

module "iam_instance_profile" {
  source      = "../ec2-ecs-iam-instance-profile"
  name_prefix = local.name
}

resource "aws_network_interface" "this" {
  subnet_id       = var.config.subnet.id
  security_groups = [aws_security_group.this.id]
}

resource "aws_instance" "this" {
  ami           = data.aws_ssm_parameter.ami_id.value
  instance_type = var.config.ec2_instance_type

  primary_network_interface {
    network_interface_id = aws_network_interface.this.id
  }

  iam_instance_profile = module.iam_instance_profile.name
  user_data_base64     = data.cloudinit_config.this.rendered

  lifecycle {
    replace_triggered_by = [terraform_data.userdata_fingerprint]
  }

  tags = {
    Name = local.name
  }
}

resource "aws_ebs_volume" "this" {
  availability_zone = var.config.subnet.availability_zone
  size              = var.config.ebs_volume_size
  type              = "gp3"
}

resource "aws_volume_attachment" "data" {
  device_name = local.ebs_data_volume.device
  volume_id   = aws_ebs_volume.this.id
  instance_id = aws_instance.this.id
}

module "ecs_task_execution_role_prometheus" {
  source      = "../ecs-task-execution-role"
  name_prefix = "${local.name}-prometheus"
  ssm_parameter_arns = []
}

module "ecs_task_execution_role_grafana" {
  source      = "../ecs-task-execution-role"
  name_prefix = "${local.name}-grafana"
  ssm_parameter_arns = [
    var.config.grafana.admin_password_arn,
  ]
}

resource "aws_cloudwatch_log_group" "prometheus" {
  name = "/ecs/${local.name}-prometheus"
}

resource "aws_cloudwatch_log_group" "grafana" {
  name = "/ecs/${local.name}-grafana"
}

resource "aws_ecs_cluster" "this" {
  name = local.name
}

resource "aws_ecs_task_definition" "prometheus" {
  family                   = "${local.name}-prometheus"
  requires_compatibilities = ["EC2"]
  network_mode             = "host"
  cpu                      = var.config.prometheus.ecs_task_cpu
  memory                   = var.config.prometheus.ecs_task_memory
  execution_role_arn       = module.ecs_task_execution_role_prometheus.arn

  container_definitions = jsonencode([
    {
      name      = "${local.name}-prometheus"
      image     = var.config.prometheus.ecs_task_container_image
      essential = true
      portMappings = [
        {
          containerPort = local.prometheus_port
          hostPort      = local.prometheus_port
          protocol      = "tcp"
        }
      ]
      environment = [
        { name = "PROMETHEUS_WEB_LISTEN_ADDRESS", value = ":${tostring(local.prometheus_port)}" },
        { name = "PROMETHEUS_STORAGE_TSDB_PATH", value = local.prometheus_docker_volume.container_path },
      ]
      mountPoints = [{
        sourceVolume  = local.prometheus_docker_volume.name
        containerPath = local.prometheus_docker_volume.container_path
        readOnly      = false
      }]
      logConfiguration = {
        logDriver = "awslogs"
        options = {
          awslogs-region        = aws_cloudwatch_log_group.prometheus.region
          awslogs-group         = aws_cloudwatch_log_group.prometheus.name
          awslogs-stream-prefix = "ecs"
        }
      }
    }
  ])

  volume {
    name      = local.prometheus_docker_volume.name
    host_path = local.prometheus_docker_volume.host_path
  }
}

resource "aws_ecs_task_definition" "grafana" {
  family                   = "${local.name}-grafana"
  requires_compatibilities = ["EC2"]
  network_mode             = "host"
  cpu                      = var.config.grafana.ecs_task_cpu
  memory                   = var.config.grafana.ecs_task_memory
  execution_role_arn       = module.ecs_task_execution_role_grafana.arn

  container_definitions = jsonencode([
    {
      name      = "${local.name}-grafana"
      image     = var.config.grafana.esc_task_container_image
      essential = true
      portMappings = [
        {
          containerPort = local.grafana_port
          hostPort      = local.grafana_port
          protocol      = "tcp"
        }
      ]
      environment = [
        { name = "GF_SERVER_HTTP_PORT", value = tostring(local.grafana_port) },
        { name = "GF_PATHS_DATA", value = local.grafana_docker_volume.container_path },
        { name = "GF_SECURITY_ADMIN_USER", value = "admin" },
        { name = "SECRETS_VERSION", value = var.config.grafana.secrets_version },
      ]
      secrets = [
        { name = "GF_SECURITY_ADMIN_PASSWORD", valueFrom = var.config.grafana.admin_password_arn }
      ]
      mountPoints = [{
        sourceVolume  = local.grafana_docker_volume.name
        containerPath = local.grafana_docker_volume.container_path
        readOnly      = false
      }]
      logConfiguration = {
        logDriver = "awslogs"
        options = {
          awslogs-region        = aws_cloudwatch_log_group.grafana.region
          awslogs-group         = aws_cloudwatch_log_group.grafana.name
          awslogs-stream-prefix = "ecs"
        }
      }
    }
  ])

  volume {
    name      = local.grafana_docker_volume.name
    host_path = local.grafana_docker_volume.host_path
  }
}

resource "aws_ecs_service" "prometheus" {
  name            = "${local.name}-prometheus"
  cluster         = aws_ecs_cluster.this.id
  task_definition = aws_ecs_task_definition.prometheus.arn
  desired_count   = 1
  launch_type     = "EC2"

  deployment_minimum_healthy_percent = 0
  deployment_maximum_percent         = 100

  depends_on = [aws_instance.this, aws_volume_attachment.data]
}

resource "aws_ecs_service" "grafana" {
  name            = "${local.name}-grafana"
  cluster         = aws_ecs_cluster.this.id
  task_definition = aws_ecs_task_definition.grafana.arn
  desired_count   = 1
  launch_type     = "EC2"

  deployment_minimum_healthy_percent = 0
  deployment_maximum_percent         = 100

  depends_on = [aws_instance.this, aws_volume_attachment.data]
}
