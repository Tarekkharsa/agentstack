variable "db_password" {
  description = "Injected from the secret store at apply time — never in VCS."
  type        = string
  sensitive   = true
}
