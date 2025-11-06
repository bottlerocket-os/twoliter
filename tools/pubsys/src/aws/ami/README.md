## AMI Publication

Pubsys provides mechanisms for publishing Bottlerocket variants as AWS EC2 AMIs.
In general, this involves:
* Creating EBS snapshots for each OS image needed for the variant.
* Registering an AMI in a "leader" region with the desired AMI attributes.
* Copying that AMI to all target regions.

## AMI Registration Controls

Pubsys selects useful defaults for AMI registration parameters based on properties of the
Bottlerocket variant; however, a Bottlerocket variant builder can override any of these
parameters using an optional document called an *amispec template*.

*amispec template*s are provided with a Bottlerocket variant by placing a file called `amispec.toml`
alongside the `Cargo.toml` file that defines the variant.
So for example, if you have a variant in your Twoliter workspace called "my-variant", you should
place the *amispec template* at `$TWOLITER_WORKSPACE/variants/my-variant/amispec.toml`.

Every value in an *amispec template* (even integral values like an EBS volume's size) can be specified
using handlebars templates given as TOML strings.

### The Default amispec Template

Whether or not a Bottlerocket variant provides their own `amispec.toml` template, Pubsys always
starts the registration process by using its default template as a base.

The default template is similar to the following:

```toml
name = "{{ ami.unique_name }}"
description = "{{ ami.description }}"
architecture = "{{ ami.arch }}"
root-device-name = "/dev/xvda"

sriov-net-support = "simple"
virtualization-type = "hvm"
ena-support = true

# These attributes are provided if the Bottlerocket variant has *uefi-secure-boot* enabled
uefi-data = "UEFI DATA GENERATED DURING BUILD PROCESS"
boot-mode = "uefi-preferred"

# Block device mapping for the root volume
[block-device-mappings."/dev/xvda".ebs]
volume-type = "gp2"
volume-size = "{{ block_devices.root.volume_size }}"
snapshot-id = "{{ block_devices.root.snapshot_id }}"
delete-on-termination = true

# Block device mapping for the data volume
# (only included if the Bottlerocket variant uses the "split" partition-plan)
[block-device-mappings."/dev/xvdb".ebs]
volume-type = "gp2"
volume-size = "{{ block_devices.data.volume_size }}"
snapshot-id = "{{ block_devices.data.snapshot_id }}"
delete-on-termination = true
```

### Overriding amispec Values for your Variant

When an *amispec template* is provided with a Bottlerocket variant, the values from that template
are merged/upserted into the default template specified above.
This means that *amispec template* users must only provide values that they wish to override.

Here are a few sample use-cases.

#### Using gp3 instead of gp2 for volumes

```toml
# Will be merged into the default /dev/xvda, overwriting the volume-type
[block-device-mappings."/dev/xvda".ebs]
volume-type = "gp3"

[block-device-mappings."/dev/xvdb".ebs]
volume-type = "gp3"
```

#### Enable NitroTPM Boot Properties

```toml
boot-mode = "uefi"
tpm-support = "v2.0"
```

#### Adding More EBS Volumes and Enforcing IMDSv2

```toml
imds-support = "v2.0"

# Adds an IO2 volume mounted as /dev/xvdc
[block-device-mappings."/dev/xvdc".ebs]
volume-type = "io2"
volume-size = 30
iops = 1000
```

#### Adding Tags to Registered AMIs

```toml
[tags]
Environment = "production"
Team = "platform"
Version = "1.0"
CostCenter = "engineering"
```

Tags can also use template variables:

```toml
[tags]
Name = "{{ ami.unique_name }}"
Architecture = "{{ ami.arch }}"
BuildDate = "2024-01-15"
```

#### Adding a Prefix to the Default AMI Name

```toml
name = "my-ami-called-{{ ami.unique_name }}"
description = "The original description is {{ ami.description}}"
```

### amispec Reference

The canonical references for fields accepted by *amispec* are:
* The [AWS documentation for RegisterImage.](https://docs.aws.amazon.com/AWSEC2/latest/APIReference/API_RegisterImage.html)
* The [AWS documentation for BlockDeviceMappings.](https://docs.aws.amazon.com/AWSEC2/latest/APIReference/API_BlockDeviceMapping.html)
* The [AWS documentation for EbsBlockDevices.](https://docs.aws.amazon.com/AWSEC2/latest/APIReference/API_EbsBlockDevice.html)

The only key difference for amispec are that fields are specified using `kebab-case` instead of
`PascalCase`.

Each of these fields can be used in an *amispec template*, with a few exceptions: "Paravirtual" and
"S3-backed" AMIs are not supported, meaning that the following fields are not respected:
* Setting `virtualization-type` to `paravirtual`
* Providing a `kernel-id`
* Providing a `ramdisk-id`
* Providing an `image-location`

### Template Variables

*amispec templates* can use any handlebars variables passed to the template by Pubsys' render context.
The render context uses the following structure:

```json
{
    "ami": {
        "unique_name": "name determined by Twoliter based on workspace",
        "description": "default AMI description",
        "arch": "AMI architecture determined by Twoliter based on workspace"
    },
    "block_devices": {
        "root": {
            "volume_size": 2,
            "snapshot_id": "snapshot-id of root snapshot registered by pubsys",
            "device_name": "/dev/xvda"
        },
        "data": {
            "volume_size": 20,
            "snapshot_id": "snapshot-id of data snapshot registered by pubsys",
            "device_name": "/dev/xvdb"
        }
    }
}
```

These default values are derived from the variant definition and current build.
Because they are added as template variables, any Bottlerocket variant builder can refer to them
in their *amispec template*, even if overwriting a value defined by the default template.


#### Example: Adding an additional EBS volume Mirroring the Root

This isn't necessarily a practical example; however, but it demonstrates how to take advantage of
the template variables provided by Pubsys.

Suppose you wanted an additional EBS volume to attach to your AMI which had the same content and
size as the root volume. You could accomplish that by adding a new EBS volume to the block device
mappings like so:

```toml
[block-device-mappings."/dev/xvdc".ebs]
volume-type = "gp3"
volume-size = "{{ block_devices.root.volume_size }}"
snapshot-id = "{{ block_devices.root.snapshot_id }}"
delete-on-termination = true
```
