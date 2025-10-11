# Test cmd
## Fib
./target/release/revmc-examples-fibonacci-bench-compare 1000 1000

first results | AOT=87571595343018854458033386304178158174356588264390370 (gas=37193) | Interpreter=87571595343018854458033386304178158174356588264390370 (gas=37193)
fib(255) n_iters=1000 | AOT avg=2.485µs total=2.485125ms | Interpreter avg=17.642µs total=17.642167ms


## erc20 transfer
./target/release/revmc-examples-erc20-transfer-use-storage-bench-compare 100 100 10

first results | AOT gas=51041 | balances A:10000.00 BNB->9999.90 BNB B:0.00 BNB->0.10 BNB | Interpreter gas=51041 | balances A:10000.00 BNB->9999.90 BNB B:0.00 BNB->0.10 BNB
erc20.transfer(use storage) batches=100 batch=100 commit_every=10 | transact-only AOT avg=10.743µs total=1.074329ms | Interpreter avg=6.947µs total=694.716µs | transact+commit AOT avg=12.184µs total=1.218457ms | Interpreter avg=8.972µs total=897.21µs
last results  | AOT balances A:7899.90 BNB B:2100.10 BNB | Interpreter balances A:7899.90 BNB B:2100.10 BNB