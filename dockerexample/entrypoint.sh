#!/bin/sh -l
set -eux

ebbflow run-blocking --loglevel Debug 8000 $1 &> ebb.log &

# Super simple server
python3.8 -m http.server 8000