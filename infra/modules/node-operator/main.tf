variable "name" {
  type = string
}

data "aws_region" "current" {}

module "vpc" {
  source  = "terraform-aws-modules/vpc/aws"
  version = "~> 6.5"

  name = "${var.name}-wcn-node-operator-vpc"
  cidr = "10.0.0.0/16"

  azs             = ["${data.aws_region.current}a", "${data.aws_region.current}b"]
  private_subnets = ["10.0.1.0/24"]
  public_subnets  = ["10.0.101.0/24", "10.0.102.0/24"]
}
