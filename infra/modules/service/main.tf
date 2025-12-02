variable "config" {
  type = object({
    name = string
    image = string
    cpu_burst = bool
    cpu = number
    memory = number
    disk = optional(number)
    public_ip = bool

    vpc = object({
      vpc_id     = string
      vpc_cidr_block = string
    })

    subnet = object({
      id = string
      availability_zone = string
    })

    ports = list(object({
      port = number
      protocol = string
      internal = bool 
    }))

    environment = map(string)

    secrets = map(object({
      ssm_parameter_arn = string
      version = number
    }))

    command = option(list(string))
  })
}

data "aws_ssm_parameter" "ami_id" {
  name = "/aws/service/ecs/optimized-ami/amazon-linux-2023/arm64/al2023-ami-ecs-hvm-2023.0.20251108-kernel-6.1-arm64/image_id"
}

locals {
  az = var.config.subnet.availability_zone
  region = substr(local.az, 0, length(local.az) - 1)

  instance_type = {
    "2cpu-1mem-burst" = "t4g.micro"
    "1cpu-2mem-normal" = "c6g.medium"
    "2cpu-4mem-normal" = "c6g.large"
  }["${var.config.cpu}cpu-${var.config.memory}mem-${var.config.cpu_burst ? "burst" : "normal"}"]
}

resource "aws_security_group" "this" {
  name   = var.config.name
  vpc_id = var.config.vpc.vpc_id
}

resource "aws_vpc_security_group_ingress_rule" "this" {
  for_each = {
    for p in var.config.ports :
    "${p.port}:${p.protocol}:${p.internal ? "internal" : "external" }" => p
  }
  
  security_group_id = aws_security_group.this.id
  cidr_ipv4         = each.value.internal ? var.config.vpc.vpc_cidr_block : "0.0.0.0/0"
  from_port         = each.value.port
  to_port           = each.value.port
  ip_protocol       = each.value.protocol
}

resource "aws_vpc_security_group_ingress_rule" "ec2_instance_connect" {
  security_group_id = aws_security_group.this.id
  cidr_ipv4         = var.config.vpc.vpc_cidr_block
  from_port         = 22
  to_port           = 22
  ip_protocol       = "tcp"
}

resource "aws_vpc_security_group_egress_rule" "all" {
  security_group_id = aws_security_group.this.id
  cidr_ipv4         = "0.0.0.0/0"
  ip_protocol       = "-1"
}

data "cloudinit_config" "this" {
  gzip = false

  part {
    filename     = "userdata.sh"
    content_type = "text/x-shellscript"
    content = templatefile("${path.module}/userdata.sh", {
      ecs_cluster     = aws_ecs_cluster.this.name
      ebs_device_path = var.config.disk == null ? "" : local.ebs_device_path
      mount_point = "/mnt/data"
    })
  }
}

resource "terraform_data" "userdata_fingerprint" {
  input = sha256(data.cloudinit_config.this.rendered)
}

resource "aws_iam_role" "this" {
  name = var.config.name
  assume_role_policy = jsonencode({
    Version   = "2012-10-17",
    Statement = [
      { Effect = "Allow", Principal = { Service = "ec2.amazonaws.com" }, Action = "sts:AssumeRole" },
      { Effect = "Allow", Principal = { Service = "ecs-tasks.amazonaws.com" }, Action = "sts:AssumeRole" },
    ]
  })
}

resource "aws_iam_role_policy_attachment" "this" {
  for_each = toset([
    "arn:aws:iam::aws:policy/service-role/AmazonEC2ContainerServiceforEC2Role",
    "arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore",
    "arn:aws:iam::aws:policy/CloudWatchLogsFullAccess",
    "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"    
  ])
  role       = aws_iam_role.this.name
  policy_arn = each.value
}

resource "aws_iam_role_policy" "ssm" {
  count = length(var.config.secrets) > 0 ? 1: 0  
  name = "ecs-exec-ssm-kms"
  role = aws_iam_role.this.id
  policy = jsonencode({
    Version = "2012-10-17",
    Statement = [
      {
        Effect = "Allow",
        Action = ["ssm:GetParameter", "ssm:GetParameters", "ssm:GetParametersByPath"],
        Resource = [for s in var.config.secrets : s.ssm_parameter_arn]
      },
      { Effect = "Allow", Action = ["kms:Decrypt"], Resource = "*" }
    ]
  })
}

resource "aws_iam_instance_profile" "this" {
  name = var.config.name
  role        = aws_iam_role.this.name
}

resource "aws_instance" "this" {
  ami           = data.aws_ssm_parameter.ami_id.value
  instance_type = local.instance_type

  primary_network_interface {
    network_interface_id = aws_network_interface.this.id
  }

  iam_instance_profile   = aws_iam_instance_profile.this.name
  user_data_base64       = data.cloudinit_config.this.rendered

  lifecycle {
    replace_triggered_by  = [terraform_data.userdata_fingerprint]
  }

  tags = {
    Name = var.config.name
  }
}

resource "aws_ebs_volume" "this" {
  count = var.config.disk != null ? 1 : 0
  availability_zone = var.config.subnet.availability_zone
  size              = var.config.disk
  type              = "gp3"
}

locals {
  ebs_device_path = "/dev/xvdh"
}

resource "aws_volume_attachment" "data" {
  count = var.config.disk != null ? 1 : 0
  device_name = local.ebs_device_path
  volume_id   = aws_ebs_volume.this[0].id
  instance_id = aws_instance.this.id
}

resource "aws_network_interface" "this" {
  subnet_id       = var.config.subnet.id
  security_groups = [aws_security_group.this.id]
}

resource "aws_eip" "this" {
  count = var.config.public_ip ? 1 : 0
  domain = "vpc"
}

resource "aws_eip_association" "this" {
  count = var.config.public_ip ? 1 : 0
  network_interface_id   = aws_network_interface.this.id
  allocation_id = aws_eip.this[0].id
}

resource "aws_cloudwatch_log_group" "this" {
  name = "/ecs/${var.config.name}"
}

resource "aws_ecs_task_definition" "this" {
  family                   = var.config.name
  requires_compatibilities = ["EC2"]
  network_mode             = "host"
  execution_role_arn       = aws_iam_role.this.arn

  container_definitions = jsonencode([
    {
      name      = var.config.name
      image     = var.config.image
      user = "1001:1001"
      command = var.config.command
      # Make sure that task doesn't require all the available memory of the instance.
      # Usually around 200-300 MBs are being used by the OS.
      # The task will be able to use more than the specified amount.
      memoryReservation = var.config.memory * 1024 / 2
      essential = true
      portMappings = [ for p in var.config.ports : {
        containerPort = p.port
        hostPort      = p.port
        protocol      = p.protocol
      }]
      # Specify secret versions as separate environment variables in order to force
      # task definition updates when secrets change.
      environment = concat(
      [ for k in sort(keys(var.config.environment)) : {
        name = k
        value = var.config.environment[k]
      }],
      [ for k, v in var.config.secrets : {
        name = "${k}_VERSION"
        value = tostring(v.version)
      }])
      secrets = [ for k, v in var.config.secrets : {
        name = k
        valueFrom = v.ssm_parameter_arn
      }]
      mountPoints = [{
        sourceVolume  = "data"
        containerPath = "/data"
        readOnly      = false
      }]
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

  dynamic volume {
    for_each = var.config.disk != 0 ? { "data" = "/mnt/data" } : {}
    content {
      name      = volume.key
      host_path = volume.value   
    }
  }
}

resource "aws_ecs_cluster" "this" {
  name = var.config.name
}

resource "aws_ecs_service" "this" {
  name            = var.config.name
  cluster         = aws_ecs_cluster.this.id
  task_definition = aws_ecs_task_definition.this.arn
  desired_count   = 1
  launch_type     = "EC2"

  deployment_minimum_healthy_percent = 0
  deployment_maximum_percent         = 100

  depends_on = [aws_instance.this]
}

output "private_ip" {
  value = aws_network_interface.this.private_ip
}
