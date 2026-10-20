# SPDX-License-Identifier: AGPL-3.0-only
# Apelora replica anti-affinity for Incus's instance placement (instances.placement.scriptlet).
#
# Replicas of one module should run on different cluster members, so a member failure takes
# at most one of them. For an instance carrying Apelora's labels, choose the candidate member
# with the fewest instances of the same application and module, then the fewest instances
# of the project, then the name (deterministic). Instances without Apelora's labels, and
# clusters with one candidate, are left to Incus's built-in placement (no set_target call).
# Applies to new instances, evacuation and relocation alike. Installed by the Incus operator
# (it is cluster-wide server configuration); see docs/design/application-deployment.md.

APPLICATION = "user.apelora.application"
MODULE = "user.apelora.module"
# Instances created before "appliance" was renamed "module" carry this key instead.
LEGACY_MODULE = "user.apelora.appliance"

def module_of(config):
    return config.get(MODULE, "") or config.get(LEGACY_MODULE, "")

def instance_placement(request, candidate_members):
    config = request.config or {}
    application = config.get(APPLICATION, "")
    module = module_of(config)
    if not application or not module or len(candidate_members) < 2:
        return

    best = None
    best_key = None
    for member in candidate_members:
        siblings = 0
        total = 0
        # Keyword arguments: on Incus 7.0.1 positional arguments returned no instances
        # (docs/development/TEST-CLUSTER.md).
        for instance in get_instances(location = member.server_name, project = request.project):
            total += 1
            labels = instance.config or {}
            if instance.name != request.name and labels.get(APPLICATION) == application and module_of(labels) == module:
                siblings += 1
        key = (siblings, total, member.server_name)
        if best_key == None or key < best_key:
            best = member.server_name
            best_key = key

    log_info("apelora placement: ", request.project, "/", request.name, " -> ", best, " (replicas there: ", best_key[0], ")")
    set_target(best)
    return
