use gdal::Dataset;
use gdal_sys::{
    CPLErr, GDALChunkAndWarpImage, GDALCreateGenImgProjTransformer2, GDALCreateWarpOperation,
    GDALCreateWarpOptions, GDALDestroyGenImgProjTransformer, GDALDestroyWarpOperation,
    GDALDestroyWarpOptions, GDALGenImgProjTransform, GDALReprojectImage, GDALResampleAlg,
    GDALWarpInitDefaultBandMapping,
};
use std::{ffi::CString, ptr};

pub enum Transform {
    Pipeline(String),
    Srs(String, String),
}

pub fn warp(source_ds: &Dataset, target_ds: &Dataset, tile_size: u16, transform: &Transform) {
    unsafe {
        let warp_options = GDALCreateWarpOptions();

        (*warp_options).eResampleAlg = GDALResampleAlg::GRA_Lanczos;

        let result = match transform {
            Transform::Pipeline(pipeline) => {
                let mut options: Vec<*mut i8> = vec![];

                options.push(
                    CString::new(format!("COORDINATE_OPERATION={pipeline}"))
                        .unwrap()
                        .into_raw(),
                );

                options.push(ptr::null_mut());

                let gen_img_proj_transformer = GDALCreateGenImgProjTransformer2(
                    source_ds.c_dataset(),
                    target_ds.c_dataset(),
                    options.as_mut_ptr(),
                );

                drop(CString::from_raw(*options.get(0).unwrap()));

                assert!(
                    !gen_img_proj_transformer.is_null(),
                    "Failed to create image projection transformer"
                );

                (*warp_options).pTransformerArg = gen_img_proj_transformer;

                (*warp_options).pfnTransformer = Some(GDALGenImgProjTransform);

                (*warp_options).hSrcDS = source_ds.c_dataset();

                (*warp_options).hDstDS = target_ds.c_dataset();

                (*warp_options).nDstAlphaBand = 0;

                (*warp_options).nSrcAlphaBand = 0;

                GDALWarpInitDefaultBandMapping(warp_options, source_ds.raster_count() as i32);

                let warp_operation = GDALCreateWarpOperation(warp_options);

                assert!(
                    !warp_operation.is_null(),
                    "Failed to create GDALCreateWarpOperation"
                );

                let result =
                    GDALChunkAndWarpImage(warp_operation, 0, 0, tile_size.into(), tile_size.into());

                if !(*warp_options).pTransformerArg.is_null() {
                    GDALDestroyGenImgProjTransformer((*warp_options).pTransformerArg);
                }

                GDALDestroyWarpOperation(warp_operation);

                result
            }
            Transform::Srs(source_wkt, target_wkt) => {
                let result = GDALReprojectImage(
                    source_ds.c_dataset(),
                    source_wkt.as_ptr() as *const i8,
                    target_ds.c_dataset(),
                    target_wkt.as_ptr() as *const i8,
                    GDALResampleAlg::GRA_Lanczos,
                    0.0,
                    0.0,
                    None,
                    ptr::null_mut(),
                    warp_options,
                );

                result
            }
        };

        GDALDestroyWarpOptions(warp_options);

        assert!(
            result == CPLErr::CE_None,
            "ChunkAndWarpImage failed with error code: {result:?}"
        );
    }
}
