variable "config" {
  type = object({
    name      = string
    image     = string
    cpu_arch  = optional(string)
    cpu_burst = optional(bool)
    cpu_cores = number
    memory    = number
    disk      = optional(number)
    public_ip = bool

    vpc = object({
      vpc_id         = string
      vpc_cidr_block = string
    })

    subnet = object({
      id                = string
      availability_zone = string
    })

    containers = list(object({
      name      = optional(string)
      image     = string
      essential = optional(bool)
      ports = list(object({
        port     = number
        protocol = string
        internal = bool
      }))

      environment = map(string)

      secrets = map(object({
        ssm_parameter_arn = string
        version           = number
      }))

      entry_point = optional(list(string))
      command     = optional(list(string))
    }))

    s3_buckets = optional(list(string))
  })
}

data "aws_ssm_parameter" "ami_id" {
  name = "/aws/service/ecs/optimized-ami/amazon-linux-2023/arm64/al2023-ami-ecs-hvm-2023.0.20251108-kernel-6.1-arm64/image_id"
}

data "aws_ssm_parameter" "x86_ami_id" {
  name = "/aws/service/ecs/optimized-ami/amazon-linux-2023/al2023-ami-ecs-hvm-2023.0.20251108-kernel-6.1-x86_64/image_id"
}

locals {
  az     = var.config.subnet.availability_zone
  region = substr(local.az, 0, length(local.az) - 1)

  cpu_arch  = coalesce(var.config.cpu_arch, "arm")
  cpu_burst = coalesce(var.config.cpu_burst, false)

  ami_id = {
    "arm" = data.aws_ssm_parameter.ami_id
    "x86" = data.aws_ssm_parameter.x86_ami_id
  }[local.cpu_arch].value

  instance_type = {
    "arm-2cpu-1mem-burst"   = "t4g.micro"
    "arm-2cpu-4mem-burst"   = "t4g.medium"
    "arm-2cpu-8mem-burst"   = "t4g.large"
    "arm-1cpu-2mem-normal"  = "c6g.medium"
    "arm-2cpu-4mem-normal"  = "c6g.large"
    "arm-4cpu-8mem-normal"  = "c6g.xlarge"
    "arm-8cpu-16mem-normal" = "c6g.2xlarge"
    "x86-2cpu-4mem-normal"  = "c5a.large"
    "x86-8cpu-16mem-normal" = "c5a.2xlarge"
  }["${local.cpu_arch}-${var.config.cpu_cores}cpu-${var.config.memory}mem-${local.cpu_burst ? "burst" : "normal"}"]

  secrets    = merge(var.config.containers[*].secrets...)
  s3_buckets = coalesce(var.config.s3_buckets, [])
}

resource "aws_security_group" "this" {
  name   = var.config.name
  vpc_id = var.config.vpc.vpc_id
}

resource "aws_vpc_security_group_ingress_rule" "this" {
  for_each = {
    for p in flatten(var.config.containers[*].ports) :
    "${p.port}:${p.protocol}:${p.internal ? "internal" : "external"}" => p
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
      mount_point     = "/mnt/data"
    })
  }
}

resource "terraform_data" "instance_type" {
  input = local.instance_type
}

resource "aws_iam_role" "this" {
  name = "${local.region}-${var.config.name}"
  assume_role_policy = jsonencode({
    Version = "2012-10-17",
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
  count = length(local.secrets) > 0 ? 1 : 0
  name  = "ecs-exec-ssm-kms"
  role  = aws_iam_role.this.id
  policy = jsonencode({
    Version = "2012-10-17",
    Statement = [
      {
        Effect   = "Allow",
        Action   = ["ssm:GetParameter", "ssm:GetParameters", "ssm:GetParametersByPath"],
        Resource = [for s in local.secrets : s.ssm_parameter_arn]
      },
      { Effect = "Allow", Action = ["kms:Decrypt"], Resource = "*" }
    ]
  })
}

resource "aws_iam_role_policy" "s3" {
  count = length(local.s3_buckets) > 0 ? 1 : 0
  name  = "ecs-exec-s3"
  role  = aws_iam_role.this.id
  policy = jsonencode({
    Version = "2012-10-17",
    Statement = [
      {
        Effect   = "Allow",
        Action   = ["s3:ListBucket"],
        Resource = [for bucket in local.s3_buckets : "arn:aws:s3:::${bucket}"]
      },
      {
        Effect = "Allow",
        Action = [
          "s3:PutObject",
          "s3:AbortMultipartUpload",
          "s3:CreateMultipartUpload",
          "s3:CompleteMultipartUpload",
          "s3:ListMultipartUploadParts",
        ],
        Resource = [for bucket in local.s3_buckets : "arn:aws:s3:::${bucket}/*"]
      },
    ]
  })
}

resource "aws_iam_instance_profile" "this" {
  name = "${local.region}-${var.config.name}"
  role = aws_iam_role.this.name
}

resource "aws_instance" "this" {
  ami           = local.ami_id
  instance_type = local.instance_type

  primary_network_interface {
    network_interface_id = aws_network_interface.this.id
  }

  iam_instance_profile        = aws_iam_instance_profile.this.name
  user_data_base64            = data.cloudinit_config.this.rendered
  user_data_replace_on_change = true

  lifecycle {
    replace_triggered_by = [
      # The default behaviour on instance_type changes is to start/stop the instance, but ECS agent breaks
      # after instance type change because of some config shennanigans.
      # We force recreation of the instance here, on every instance_type change.
      terraform_data.instance_type,
    ]
  }

  tags = {
    Name = var.config.name
  }
}

resource "aws_ebs_volume" "this" {
  count             = var.config.disk != null ? 1 : 0
  availability_zone = var.config.subnet.availability_zone
  size              = var.config.disk
  type              = "gp3"
}

locals {
  ebs_device_path = "/dev/xvdh"
}

resource "aws_volume_attachment" "data" {
  count       = var.config.disk != null ? 1 : 0
  device_name = local.ebs_device_path
  volume_id   = aws_ebs_volume.this[0].id
  instance_id = aws_instance.this.id
}

resource "aws_network_interface" "this" {
  subnet_id       = var.config.subnet.id
  security_groups = [aws_security_group.this.id]
}

resource "aws_eip" "this" {
  count  = var.config.public_ip ? 1 : 0
  domain = "vpc"
}

resource "aws_eip_association" "this" {
  count                = var.config.public_ip ? 1 : 0
  network_interface_id = aws_network_interface.this.id
  allocation_id        = aws_eip.this[0].id
}

resource "aws_cloudwatch_log_group" "this" {
  name = "/ecs/${var.config.name}"
}

resource "aws_ecs_task_definition" "this" {
  family                   = var.config.name
  requires_compatibilities = ["EC2"]
  network_mode             = "host"
  execution_role_arn       = aws_iam_role.this.arn

  container_definitions = jsonencode([for i in range(length(var.config.containers)) :
    {
      name       = coalesce(var.config.containers[i].name, var.config.name)
      image      = var.config.containers[i].image
      user       = "1001:1001"
      entryPoint = var.config.containers[i].entry_point
      command    = var.config.containers[i].command
      # Make sure that the primary task doesn't require all the available memory of the instance.
      # Usually around 200-300 MBs are being used by the OS.
      # For sidecards reserve the minimum possible amount (6 MB).
      # The tasks will be able to use more than the specified amount.
      memoryReservation = i == 0 ? var.config.memory * 1024 / 2 : 6
      essential         = coalesce(var.config.containers[i].essential, true)
      portMappings = [for p in var.config.containers[i].ports : {
        containerPort = p.port
        hostPort      = p.port
        protocol      = p.protocol
      }]
      # Specify secret versions as separate environment variables in order to force
      # task definition updates when secrets change.
      environment = concat(
        [for k in sort(keys(var.config.containers[i].environment)) : {
          name  = k
          value = var.config.containers[i].environment[k]
        }],
        [for k, v in var.config.containers[i].secrets : {
          name  = "${k}_VERSION"
          value = tostring(v.version)
      }])
      secrets = [for k, v in var.config.containers[i].secrets : {
        name      = k
        valueFrom = v.ssm_parameter_arn
      }]
      mountPoints = var.config.disk == null ? [] : [{
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

  dynamic "volume" {
    for_each = var.config.disk != null ? { "data" = "/mnt/data" } : {}
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

  force_delete = true

  # If we don't order resources this way, terraform may be unable to delete the ECS service
  # due to some necessary resources being deleted before the service itself.
  depends_on = [
    aws_instance.this,
    aws_volume_attachment.data,
    aws_eip_association.this,
    aws_iam_role_policy_attachment.this,
    aws_iam_role_policy.ssm,
    aws_vpc_security_group_egress_rule.all,
    aws_vpc_security_group_ingress_rule.this,
  ]
}

output "private_ip" {
  value = aws_network_interface.this.private_ip
}
