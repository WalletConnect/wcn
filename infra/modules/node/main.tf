variable "config" {
  type = object({
    index = number
    operator_name = string

    vpc = object({
      vpc_id     = string
      vpc_cidr_block = string
    })

    subnet = object({
      id = string
      availability_zone = string
    })

    ec2_instance_type = string

    ecs_task_container_image = string
    ecs_task_cpu             = number
    ecs_task_memory          = number

    secret_key_arn = string

    primary_rpc_server_port   = number
    secondary_rpc_server_port = number
    metrics_server_port       = number

    database_rpc_server_address = string
    database_peer_id = string
    database_primary_rpc_server_port = number
    database_secondary_rpc_server_port = number

    smart_contract_address = string
    smart_contract_signer_private_key_arn = string
    smart_contract_encryption_key_arn = string
    rpc_provider_url_arn = string
 
    # Force ECS task update when secrets change
    secrets_version = string
  })
}

data "aws_ssm_parameter" "ami_id" {
  name = "/aws/service/ecs/optimized-ami/amazon-linux-2023/arm64/al2023-ami-ecs-hvm-2023.0.20251108-kernel-6.1-arm64/image_id"
}

locals {
  name = "${var.config.operator_name}-node-${var.config.index + 1}"

  az = var.config.subnet.availability_zone
  region = substr(local.az, 0, length(local.az) - 1)
}

resource "aws_security_group" "this" {
  name   = local.name
  vpc_id = var.config.vpc.vpc_id

  ingress {
    description = "Primary RPC server"
    from_port   = var.config.primary_rpc_server_port
    to_port     = var.config.primary_rpc_server_port
    protocol    = "udp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  ingress {
    description = "Secondary RPC server"
    from_port   = var.config.secondary_rpc_server_port
    to_port     = var.config.secondary_rpc_server_port
    protocol    = "udp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  ingress {
    description = "Prometheus metrics server"
    from_port   = var.config.metrics_server_port
    to_port     = var.config.metrics_server_port
    protocol    = "tcp"
    cidr_blocks = [var.config.vpc.vpc_cidr_block]
  }

  ingress {
    description = "EC2 Instance Connect"
    from_port = 22
    to_port = 22
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

resource "aws_instance" "this" {
  ami           = data.aws_ssm_parameter.ami_id.value
  instance_type = var.config.ec2_instance_type

  primary_network_interface {
    network_interface_id = aws_network_interface.this.id
  }

  iam_instance_profile   = module.iam_instance_profile.name
  user_data_base64       = data.cloudinit_config.this.rendered

  lifecycle {
    replace_triggered_by  = [terraform_data.userdata_fingerprint]
  }

  tags = {
    Name = local.name
  }
}

resource "aws_network_interface" "this" {
  subnet_id       = var.config.subnet.id
  security_groups = [aws_security_group.this.id]
}

resource "aws_eip" "this" {
  domain = "vpc"
}

resource "aws_eip_association" "this" {
  network_interface_id   = aws_network_interface.this.id
  allocation_id = aws_eip.this.id
}

module "ecs_task_execution_role" {
  source             = "../ecs-task-execution-role"
  name_prefix        = local.name
  ssm_parameter_arns = [
    var.config.secret_key_arn,
    var.config.smart_contract_signer_private_key_arn,
    var.config.smart_contract_encryption_key_arn,
    var.config.rpc_provider_url_arn,
  ]
}

resource "aws_cloudwatch_log_group" "this" {
  name = "/ecs/${local.name}"
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
        { name = "PRIMARY_RPC_SERVER_PORT", value = tostring(var.config.primary_rpc_server_port) },
        { name = "SECONDARY_RPC_SERVER_PORT", value = tostring(var.config.secondary_rpc_server_port) },
        { name = "METRICS_SERVER_PORT", value = tostring(var.config.metrics_server_port) },
        { name = "DATABASE_RPC_SERVER_ADDRESS", value = var.config.database_rpc_server_address },
        { name = "DATABASE_PEER_ID", value = var.config.database_peer_id },
        { name = "DATABASE_PRIMARY_RPC_SERVER_PORT", value = tostring(var.config.database_primary_rpc_server_port) },
        { name = "DATABASE_SECONDARY_RPC_SERVER_PORT", value = tostring(var.config.database_secondary_rpc_server_port) },
        { name = "SMART_CONTRACT_ADDRESS", value = var.config.smart_contract_address },
        { name = "SECRETS_VERSION", value = var.config.secrets_version },
      ]
      secrets = [
        { name = "SECRET_KEY", valueFrom = var.config.secret_key_arn },
        { name = "SMART_CONTRACT_SIGNER_PRIVATE_KEY", valueFrom = var.config.smart_contract_signer_private_key_arn },
        { name = "SMART_CONTRACT_ENCRYPTION_KEY", valueFrom = var.config.smart_contract_encryption_key_arn },
        { name = "RPC_PROVIDER_URL", valueFrom = var.config.rpc_provider_url_arn },
      ]
      logConfiguration = {
        logDriver = "awslogs"
        options = {
          awslogs-region        = aws_cloudwatch_log_group.this.region
          awslogs-group         = aws_cloudwatch_log_group.this.name
          awslogs-stream-prefix = "ecs"
        }
      }
    }
  ])
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

  depends_on = [aws_instance.this]
}

