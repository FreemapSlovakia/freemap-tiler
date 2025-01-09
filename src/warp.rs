use gdal::Dataset;
use gdal_sys::{
    CPLErr, GDALChunkAndWarpImage, GDALCreateGenImgProjTransformer2, GDALCreateWarpOperation,
    GDALCreateWarpOptions, GDALDestroyGenImgProjTransformer, GDALDestroyWarpOperation,
    GDALDestroyWarpOptions, GDALGenImgProjTransform, GDALResampleAlg,
    GDALWarpInitDefaultBandMapping,
};
use std::{ffi::CString, ptr};

pub fn warp(source_ds: &Dataset, target_ds: &Dataset, tile_size: u16, pipeline: Option<&str>) {
    unsafe {
        let warp_options = GDALCreateWarpOptions();

        if let Some(pipeline) = pipeline {
            let option = CString::new(format!("COORDINATE_OPERATION={pipeline}")).unwrap();

            let mut options: Vec<*mut i8> = vec![option.into_raw(), ptr::null_mut()];

            let gen_img_proj_transformer = GDALCreateGenImgProjTransformer2(
                source_ds.c_dataset(),
                target_ds.c_dataset(),
                options.as_mut_ptr(),
            );

            assert!(
                !gen_img_proj_transformer.is_null(),
                "Failed to create image projection transformer"
            );

            (*warp_options).pTransformerArg = gen_img_proj_transformer;

            (*warp_options).pfnTransformer = Some(GDALGenImgProjTransform);
        }

        (*warp_options).eResampleAlg = GDALResampleAlg::GRA_Lanczos;

        (*warp_options).hSrcDS = source_ds.c_dataset();

        (*warp_options).hDstDS = target_ds.c_dataset();

        (*warp_options).nDstAlphaBand = 0;

        (*warp_options).nSrcAlphaBand = 0;

        GDALWarpInitDefaultBandMapping(warp_options, 3);

        let warp_operation = GDALCreateWarpOperation(warp_options);

        let result =
            GDALChunkAndWarpImage(warp_operation, 0, 0, tile_size.into(), tile_size.into());

        if (*warp_options).pTransformerArg.is_null() {
            GDALDestroyGenImgProjTransformer((*warp_options).pTransformerArg);
        }

        GDALDestroyWarpOperation(warp_operation);

        GDALDestroyWarpOptions(warp_options);

        assert!(
            result == CPLErr::CE_None,
            "ChunkAndWarpImage failed with error code: {result:?}"
        );
    }
}
