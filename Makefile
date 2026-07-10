COMPOSE := docker compose --env-file .env -f docker/compose.yml
COMPOSE_GPU := $(COMPOSE) -f docker/compose.gpu.yml
GPU ?= 0
SERVICE ?= agent_container
SHELL_BIN = $(if $(filter ollama,$(SERVICE)),bash,zsh)

.DEFAULT_GOAL := help
.PHONY: help build up up-gpu down in ps restart restart-gpu lint ci test

help:
	@echo "Usage: make <target>"
	@echo "  build        Dockerイメージをビルド"
	@echo "  up           コンテナ起動（CPUのみ）"
	@echo "  up-gpu       コンテナ起動（NVIDIA GPU使用。GPU=<id> で指定、デフォルト 0）"
	@echo "  down         コンテナ停止・削除"
	@echo "  in           コンテナに入る（SERVICE=<name>、デフォルト agent_container）"
	@echo "  ps           コンテナ状態を表示"
	@echo "  restart      コンテナ再起動（CPUモード）"
	@echo "  restart-gpu  コンテナ再起動（GPUモード。GPU=<id> で指定、デフォルト 0）"

.env:
	@echo "Creating .env file..."
	@echo "USER=$$(id -un)" > .env
	@echo "USER_ID=$$(id -u)" >> .env
	@echo "GROUP=$$(id -gn)" >> .env
	@echo "GROUP_ID=$$(id -g)" >> .env
	@echo "HOME=$(HOME)" >> .env
	@echo "CONTAINER_NAME=agent_container" >> .env

build: .env
	$(COMPOSE) build \
		--build-arg UID=$$(id -u) \
		--build-arg GID=$$(id -g) \
		--build-arg USER=$$(id -un) \
		--build-arg GROUP=$$(id -gn)

up: .env
	mkdir -p $(HOME)/.ollama-transpaper
	$(COMPOSE) up -d

up-gpu: .env
	mkdir -p $(HOME)/.ollama-transpaper
	@echo "GPU mode enabled (NVIDIA_VISIBLE_DEVICES=$(GPU))"
	GPU=$(GPU) $(COMPOSE_GPU) up -d

down:
	$(COMPOSE) down

in:
	$(COMPOSE) exec -it $(SERVICE) $(SHELL_BIN)

ps:
	$(COMPOSE) ps -a

restart: down up

restart-gpu: down up-gpu

lint:
	uv run ruff check . --fix
	uv run ruff format .

ci: lint
	wrkflw validate
	# wrkflw run --verbose --runtime docker .github/workflows/format.yml TODO: enable when wrkflw supports fix mode

test:
	wget https://arxiv.org/pdf/1706.03762 -O attention_is_all_you_need.pdf
	uv run main.py --input attention_is_all_you_need.pdf --output translated_attention_is_all_you_need.pdf --model idx
