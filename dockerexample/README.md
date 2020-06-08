This docker image shows how you can run Ebbflow from inside a docker container.

The basis for this example is the `ebbflow run-blocking` command, which instead of using the background daemon, simply runs the same proxy code in a blocking manner.

Some notes:
- Of highest importance, `ebbflow run-blocking` needs a Host Key to authenticate with Ebbflow. `run-blocking` looks for the `EBB_KEY` environment variable. You can create keys on the https://ebbflow.io website in the IAM console.
- The command prints output, at various log levels (which you can specify), so redirecting this to a file is encouraged, thus `&> ebb.log`.
- This command needs to be run in the background because it is blocking, thus the trailing `&`.
- `run-blocking` does NOT require to be ran as the `ROOT` user, and the example shows this running as the user `toby`.

Example Execution for endpoint `myendpoint.com`:

```
docker run --rm -it --env EBB_KEY=ebb_hst_ffvxa3aGo6iFJrmCHXKTXPy3i5FQIIaahexample . myendpoint.com
```
