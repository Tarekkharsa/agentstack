# Production infrastructure — no agent edits. FAKE, illustrative only.
terraform {
  backend "s3" {
    bucket = "acme-billing-prod-tfstate-FAKE"
    key    = "prod/terraform.tfstate"
    region = "us-east-1"
  }
}

resource "aws_db_instance" "billing_prod" {
  identifier        = "acme-billing-prod"
  engine            = "postgres"
  instance_class    = "db.r6g.xlarge"
  allocated_storage = 500
  multi_az          = true
}
