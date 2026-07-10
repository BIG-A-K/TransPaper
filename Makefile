CONTAINER := ./docker/container.sh
.DEFAULT_GOAL := help
.PHONY: help build up down in ps restart

help:
	$(CONTAINER) help

build:
	$(CONTAINER) build

up:
	$(CONTAINER) up

down:
	$(CONTAINER) down

in:
	$(CONTAINER) in $(SERVICE)

ps:
	$(CONTAINER) ps

restart:
	$(CONTAINER) restart

lint:
	uv run ruff check . --fix
	uv run ruff format .

ci: lint
	wrkflw validate	
	# wrkflw run --verbose --runtime docker .github/workflows/format.yml TODO: enable when wrkflw supports fix mode

test:
	wget https://arxiv.org/pdf/1706.03762 -O attention_is_all_you_need.pdf
	uv run main.py --input attention_is_all_you_need.pdf --output translated_attention_is_all_you_need.pdf --model idx
