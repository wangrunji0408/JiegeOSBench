.PHONY: build run start stop test verify
build:
	python3 tools/run.py build
run:
	python3 tools/run.py run
start:
	python3 tools/run.py start
stop:
	python3 tools/run.py stop
test:
	python3 tools/test_http.py
verify:
	python3 tools/verify_binary.py
