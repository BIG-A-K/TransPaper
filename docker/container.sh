#!/bin/bash -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COMPOSE="${SCRIPT_DIR}/compose.yml"
ENV_FILE="${SCRIPT_DIR}/../.env"
CONTAINER_NAME="agent_container"
# .envファイルがない場合は作成
if [ ! -f ${ENV_FILE} ]; then
  echo "env file not found. Creating a new one."
  echo "Creating .env file..."
  echo USER=$(id -un) > ${ENV_FILE}
  echo USER_ID=$(id -u) >> ${ENV_FILE}
  echo GROUP=$(id -gn) >> ${ENV_FILE}
  echo GROUP_ID=$(id -g) >> ${ENV_FILE}
  echo "CONTAINER_NAME=${CONTAINER_NAME}" >> ${ENV_FILE}
  echo "Created .env file with the following content:"
  cat ${ENV_FILE}
fi

if [ -f ${ENV_FILE} ]; then
  export $(grep -v '^#' ${ENV_FILE} | xargs)
  export CONTAINER_NAME=${CONTAINER_NAME:-agent_container}
  echo "container : $CONTAINER_NAME"
else
  echo "ERROR!!! ： .env file not found. Please create it."
  exit 1
fi

function up() {
  if [ -f ${ENV_FILE} ]; then
    export $(grep -v '^#' ${ENV_FILE} | xargs)
  fi
  echo "Starting container with GPU: ${GPU}"
  docker compose -f ${COMPOSE} up -d
}

function in_container() {
  docker compose -f ${COMPOSE} exec -it ${CONTAINER_NAME} zsh
}

function down() {
  docker compose -f ${COMPOSE} down
}

function restart() {
  echo "Restarting container with current settings..."
  down
  up
}
function build(){
  docker compose -f ${COMPOSE} build\
    --build-arg UID=${USER_ID} \
    --build-arg GID=${GROUP_ID} \
    --build-arg USER=${USER} \
    --build-arg GROUP=${GROUP}
}
function ps() {
  docker compose -f ${COMPOSE} ps -a
}

function help() {
  echo "Usage: $0 [up|in|down|restart|build|ps|help]"
  echo "up: Start the Docker container"
  echo "in: Enter the Docker container"
  echo "down: Stop and remove the Docker container"
  echo "restart: Restart the Docker container with current settings"
  echo "build: Build the Docker image"
  echo "ps: Show the status of the Docker container"
  echo "help: Show this help message"
  echo ""
}

echo "Hello!"
if [ -z "$1" ]; then
  echo "What do you want to do?"
  echo "1.build: Build docker image"
  echo "2.up:    Up container"
  echo "3.in:    In container"
  echo "4.down:  Down container"
  echo "5.ps:    Show container status"
  echo "6.restart: Restart container"
  read -p "Enter your choice: " CHOICE
  case $CHOICE in
    1) build ; exit 0 ;;
    2) up ; exit 0 ;;
    3) in_container ; exit 0 ;;
    4) down ; exit 0 ;;
    5) ps ; exit 0 ;;
    6) restart ; exit 0 ;;
    7) help ; exit 0 ;;
    *) echo "Invalid input." ;exit 1 ;;
  esac
else
  CHOICE=$1
fi

case "$CHOICE" in
  "up") up ;;
  "in") in_container ;;
  "down") down ;;
  "restart") restart ;;
  "build") build ;;
  "ps") ps ;;
  "help") help ;;
  *) echo "$CHOICE is invalid." ; help ;;
esac
