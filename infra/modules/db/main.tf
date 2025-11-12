variable "config" {
  type = object({
    operator_name = string

    vpc = object({
      vpc_id     = string
      vpc_cidr_block = string
    })

    subnet = object({
      availability_zone = string
    })

    ec2_instance_type = string

    ebs_volume_size = number

    ecs_task_container_image = string
    ecs_task_cpu             = number
    ecs_task_memory          = number

    primary_rpc_server_port   = number
    secondary_rpc_server_port = number
    metrics_server_port       = number

    secret_key_arn = string

    # Force ECS task update when secrets change
    secrets_version = string
  })
}

locals {
  name = "${var.config.operator_name}-db"

  az = var.config.subnet.availability_zone
  region = substr(local.az, 0, length(local.az) - 1)

  data_volume = {
    name            = "data"
    ebs_device_path = "/dev/xvdh"
    host_path       = "/mnt/data"
    container_path  = "/data"
  }
}

module "ecs_task_execution_role" {
  source             = "../ecs-task-execution-role"
  name_prefix        = local.name
  ssm_parameter_arns = [var.config.secret_key_arn]
}

resource "aws_ecs_task_definition" "this" {
  family                   = local.name
  requires_compatibilities = ["EC2"]
  network_mode             = "host"
  cpu                      = var.config.ecs_task_cpu
  memory                   = var.config.ecs_task_memory
  execution_role_arn       = module.ecs_task_execution_role.arn

  container_definitions = jsonencode([
    {
      name      = local.name
      image     = var.config.ecs_task_container_image
      essential = true
      portMappings = [
        {
          containerPort = var.config.primary_rpc_server_port
          hostPort      = var.config.primary_rpc_server_port
          protocol      = "udp"
        },
        {
          containerPort = var.config.secondary_rpc_server_port
          hostPort      = var.config.secondary_rpc_server_port
          protocol      = "udp"
        },
        {
          containerPort = var.config.metrics_server_port
          hostPort      = var.config.metrics_server_port
          protocol      = "tcp"
        },
      ]
      environment = [
        { name = "PRIMARY_RPC_SERVER_PORT", value = "${var.config.primary_rpc_server_port}" },
        { name = "SECONDARY_RPC_SERVER_PORT", value = "${var.config.secondary_rpc_server_port}" },
        { name = "METRICS_SERVER_PORT", value = "${var.config.metrics_server_port}" },
        { name = "ROCKSDB_DIR", value = "/data" },
        { name = "SECRETS_VERSION", value = var.config.secrets_version },
      ]
      secrets = [
        { name = "SECRET_KEY", valueFrom = var.config.secret_key_arn }
      ]
      mountPoints = [{
        sourceVolume  = local.data_volume.name
        containerPath = local.data_volume.container_path
        readOnly      = false
      }]
      logConfiguration = {
        logDriver = "awslogs"
        options = {
          awslogs-region        = var.vpc.region
          awslogs-group         = "/ecs/${local.name}"
          awslogs-stream-prefix = "ecs"
        }
      }
    }
  ])

  volume {
    name      = local.data_volume.name
    host_path = local.data_volume.host_path
  }
}

resource "aws_ecs_cluster" "this" {
  name = local.name
}

resource "aws_ecs_service" "this" {
  name            = local.name
  cluster         = aws_ecs_cluster.this.id
  task_definition = aws_ecs_task_definition.this.arn
  desired_count   = 1
  launch_type     = "EC2"

  deployment_minimum_healthy_percent = 0
  deployment_maximum_percent         = 100
}

data "aws_ssm_parameter" "ami" {
  name = "/aws/service/ecs/optimized-ami/amazon-linux-2/amzn2-ami-ecs-hvm-2.0.20251015-x86_64-ebs"
}

resource "aws_security_group" "this" {
  name   = local.name
  vpc_id = var.config.vpc.vpc_id

  ingress {
    description = "Primary RPC server"
    from_port   = var.config.primary_rpc_server_port
    to_port     = var.config.primary_rpc_server_port
    protocol    = "udp"
    cidr_blocks = [var.config.vpc.vpc_cidr_block]
  }

  ingress {
    description = "Secondary RPC server"
    from_port   = var.config.secondary_rpc_server_port
    to_port     = var.config.secondary_rpc_server_port
    protocol    = "udp"
    cidr_blocks = [var.config.vpc.vpc_cidr_block]
  }

  ingress {
    description = "Prometheus metrics server"
    from_port   = var.config.metrics_server_port
    to_port     = var.config.metrics_server_port
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
      ebs_device_path = local.data_volume.ebs_device_path
      mount_point     = local.data_volume.host_path
    })
  }
}

module "iam_instance_profile" {
  source      = "../ec2-ecs-iam-instance-profile"
  name_prefix = local.name
}

resource "aws_instance" "this" {
  ami           = data.aws_ssm_parameter.ami.value
  instance_type = var.config.ec2_instance_type
  subnet_id     = var.config.vpc_subnet_id

  vpc_security_group_ids = [aws_security_group.this.id]
  iam_instance_profile   = module.iam_instance_profile.name
  user_data_base64       = data.cloudinit_config.this.rendered
}

resource "aws_eip" "this" {
  domain = "vpc"
}

resource "aws_eip_association" "this" {
  instance_id   = aws_instance.this.id
  allocation_id = aws_eip.this.id
}

resource "aws_ebs_volume" "this" {
  availability_zone = var.config.subnet.availability_zone
  size              = var.config.ebs_volume_size
  type              = "gp3"
}

resource "aws_volume_attachment" "data" {
  device_name = local.data_volume.ebs_device_name
  volume_id   = aws_ebs_volume.this.id
  instance_id = aws_instance.this.id
}
