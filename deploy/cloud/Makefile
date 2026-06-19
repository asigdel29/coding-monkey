# =============================================================================
#  File: deploy/cloud/Makefile
#  Purpose: thin wrappers over the Ansible playbooks that stand up the personal-
#           cloud strong tier. Secrets come from an ansible-vault file; pass the
#           password with VAULT="--ask-vault-pass" (or a --vault-password-file).
#
#  Typical first run:  make provision deploy connect
#  Tear down serving:  make down
#  History
#  Date         Author          Changes
#  2026-06-19   Anubhav Sigdel  initial — provision/deploy/connect/down targets
# =============================================================================

ANSIBLE ?= ansible-playbook
INVENTORY ?= ansible/inventory.ini
VAULT ?= --ask-vault-pass
PLAYBOOK = $(ANSIBLE) -i $(INVENTORY) $(VAULT)

.PHONY: provision deploy connect down all check

## Provision the colo GPU box (docker, nvidia toolkit, firewall, wireguard).
provision:
	$(PLAYBOOK) ansible/provision-gpu.yml

## Download weights and bring up the swapping gateway.
deploy:
	$(PLAYBOOK) ansible/deploy-models.yml

## Join the Pi to the VPN and repoint its orchestrator at the cloud tier.
connect:
	$(PLAYBOOK) ansible/configure-pi.yml

## Stop serving and free the GPUs (weights are kept on the volume).
down:
	$(PLAYBOOK) ansible/teardown.yml

## Full stand-up.
all: provision deploy connect

## Syntax-check every playbook without touching hosts.
check:
	@for p in ansible/*.yml; do $(ANSIBLE) -i $(INVENTORY) --syntax-check $$p; done
